// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tracking pictures: the pictures on the web a message carries to learn that it was opened,
//! not to be looked at. They are found before anything is asked for, and are never asked for
//! unless the owner says "Load anyway" for the message (always on, whatever the owner said of
//! pictures in general).
//!
//! Only strong signals make a picture a tracker:
//!
//! - its address is a known open-tracking address of a mail service ([`KNOWN`], a reviewable
//!   table);
//! - it says it is 3 pixels or less across or down (attributes or inline style), or its style
//!   hides it (`display:none`, `visibility:hidden`, `opacity:0`, a zero `max-width` or
//!   `max-height`) or puts it far off the page;
//! - a long, random-looking token in its address (one per recipient) counts only together with
//!   more: it is the sole picture from its host, it is the last picture of the body, it has no
//!   description and it declares no size of a picture meant to be seen. A token alone never
//!   blocks: a CDN picture with a hashed name is a picture.
//!
//! Everything here is pure text work over the message's HTML, bounded ([`MAX_SCAN_BYTES`],
//! [`MAX_IMAGES`]); nothing is fetched, run or kept.

use std::collections::HashSet;

/// How much of a message's HTML is looked through for pictures.
pub const MAX_SCAN_BYTES: usize = 4 * 1024 * 1024;
/// Most `<img>` tags looked at in one message.
pub const MAX_IMAGES: usize = 1_000;
/// Most trackers kept for one message.
pub const MAX_TRACKERS: usize = 200;
/// Shortest run of characters that can be a per-recipient token.
const TOKEN_MIN: usize = 24;
/// A picture that says it is at least this big is meant to be seen.
const SEEN_PX: f32 = 16.0;
/// A picture further than this off the left or the top of the page is not meant to be seen.
const OFF_PAGE_PX: f32 = -100.0;

/// Known open-tracking addresses: a host (that host or any host under it) and the beginning of
/// a path (empty: any path on that host), compared without regard to case. Each entry is the
/// address a mail service puts its "opened" pixel at; the services' own picture hosts (logos,
/// product shots) are deliberately not here.
pub const KNOWN: &[(&str, &str)] = &[
    // Mailchimp: https://<dc>.list-manage.com/track/open.php?u=…&id=…&e=…
    ("list-manage.com", "/track/open"),
    // SendGrid (Twilio): https://u123.ct.sendgrid.net/wf/open?upn=… (branded hosts: see `sendgrid_like`).
    ("sendgrid.net", "/wf/open"),
    // HubSpot: open pixels on its tracking hosts (its picture CDN, hubspotusercontent, is not one).
    ("hubspotemail.net", ""),
    ("hubspotlinks.com", ""),
    ("track.hubspot.com", ""),
    // Mailgun: https://email.<domain>/o/<token> on its own tracking host.
    ("mailgun.org", "/o/"),
    ("mailgun.net", "/o/"),
    // Mandrill (Mailchimp Transactional): https://mandrillapp.com/track/open.php?u=…
    ("mandrillapp.com", "/track/open"),
    // SparkPost: https://spgo.io/q/…, https://<x>.sparkpostmail.com/q/…
    ("sparkpostmail.com", "/q/"),
    ("spgo.io", "/q/"),
    // Postmark: https://<x>.pstmrk.it/open?…
    ("pstmrk.it", "/open"),
    // Customer.io: https://track.customeriomail.com/e/o/<token>
    ("customeriomail.com", "/e/o/"),
    ("customer.io", "/e/o/"),
    // Klaviyo: https://trk.klaviyomail.com/open/…, https://ctrk.klclick.com/o/…
    ("klaviyomail.com", "/open"),
    ("klclick.com", "/o/"),
    ("klclick1.com", "/o/"),
    // Braze: open pixels on its sending hosts.
    ("braze.com", "/o/"),
    ("braze.eu", "/o/"),
    // Salesforce Marketing Cloud (ExactTarget): https://click.<x>.com/open.aspx?…, cl.exct.net/open.aspx
    ("exct.net", "/open.aspx"),
    ("exacttarget.com", "/open.aspx"),
    // Constant Contact: https://r20.rs6.net/on.jsp?…
    ("rs6.net", "/on.jsp"),
    // Campaign Monitor: https://<x>.cmail19.com/t/<…>/o.gif and the like.
    ("cmail19.com", "/t/"),
    ("cmail20.com", "/t/"),
    ("createsend1.com", "/t/"),
    // ActiveCampaign: https://<x>.activehosted.com/lt.php?…&l=open, https://<x>.acemlna.com/lt.php
    ("activehosted.com", "/lt.php"),
    ("acemlna.com", "/lt.php"),
    ("acemlnb.com", "/lt.php"),
    // Brevo (Sendinblue): open pixels on its tracking hosts.
    ("sendibt2.com", ""),
    ("sendibt3.com", ""),
    ("sendibw.com", ""),
    ("sendibm1.com", ""),
    ("brevo.com", "/tr/op"),
    // Amazon SES: https://<x>.r.<region>.awstrack.me/I0/<token>
    ("awstrack.me", "/i0/"),
    // Google Analytics in mail: https://www.google-analytics.com/collect?v=1&t=event…
    ("google-analytics.com", "/collect"),
    ("google-analytics.com", "/__utm.gif"),
    ("google-analytics.com", "/g/collect"),
    // Facebook pixel: https://www.facebook.com/tr?id=…&ev=…
    ("facebook.com", "/tr"),
];

/// An `<img>` as written: what tells a tracker from a picture.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Img {
    pub src: String,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub style: String,
    pub alt: String,
}

/// The host (in lower case, without user, port or trailing dot), the path and the query of a
/// web address; `None` when it is not one.
fn parts(url: &str) -> Option<(String, &str, &str)> {
    let url = url.trim();
    let rest = if let Some(at) = url.find("://") {
        let scheme = &url[..at];
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return None;
        }
        &url[at + 3..]
    } else {
        url.strip_prefix("//")?
    };
    let rest = rest.split('#').next().unwrap_or_default();
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let after = &rest[end..];
    let (path, query) = after.split_once('?').unwrap_or((after, ""));
    Some((host, path, query))
}

/// Whether `host` is `name` or a host under it.
fn host_is(host: &str, name: &str) -> bool {
    host == name || host.strip_suffix(name).is_some_and(|before| before.ends_with('.'))
}

/// The address of one of the mail services' "opened" pixels ([`KNOWN`]).
pub fn known(url: &str) -> bool {
    let Some((host, path, query)) = parts(url) else { return false };
    let path = path.to_ascii_lowercase();
    KNOWN.iter().any(|(name, start)| host_is(&host, name) && path.starts_with(start)) || sendgrid_like(&path, query)
}

/// SendGrid's open pixel on a sender's own branded host: `/wf/open?upn=…`, a shape nothing else uses.
fn sendgrid_like(path: &str, query: &str) -> bool {
    path == "/wf/open" && query.split('&').any(|pair| pair.starts_with("upn="))
}

/// A number of pixels as an attribute or a style value says it: `1`, `1px`, ` 0.5 `; `None` for
/// a percentage, `auto` or anything else.
fn px(value: &str) -> Option<f32> {
    let value = value.trim().trim_end_matches("!important").trim();
    let value = value.strip_suffix("px").unwrap_or(value).trim();
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit() || b == b'.' || b == b'-' || b == b'+') {
        return None;
    }
    value.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// An inline style as `(name, value)` pairs, names in lower case, values trimmed and in lower case.
fn declarations(style: &str) -> impl Iterator<Item = (String, String)> + '_ {
    style.split(';').filter_map(|d| {
        let (name, value) = d.split_once(':')?;
        let value = value.trim().trim_end_matches("!important").trim();
        Some((name.trim().to_ascii_lowercase(), value.to_ascii_lowercase()))
    })
}

/// What the picture says of its size, by attribute or else by inline style: (width, height).
fn declared(img: &Img) -> (Option<f32>, Option<f32>) {
    let (mut width, mut height) = (img.width, img.height);
    for (name, value) in declarations(&img.style) {
        match name.as_str() {
            "width" if width.is_none() => width = px(&value),
            "height" if height.is_none() => height = px(&value),
            _ => {}
        }
    }
    (width, height)
}

/// Strong signal: the picture is too small to see, hidden by its style, or placed off the page.
pub fn unseen(img: &Img) -> bool {
    let tiny = |v: Option<f32>| v.is_some_and(|v| v <= 3.0);
    let (width, height) = declared(img);
    if tiny(width) || tiny(height) {
        return true;
    }
    // A negative offset takes off the page only what is positioned; a margin or an indent that
    // far takes anything.
    let (mut positioned, mut offset, mut margin) = (false, false, false);
    for (name, value) in declarations(&img.style) {
        let far = |limit: f32| px(&value).is_some_and(|v| v <= limit);
        match name.as_str() {
            "max-width" | "max-height" if far(0.0) => return true,
            "display" if value == "none" => return true,
            "visibility" if value == "hidden" || value == "collapse" => return true,
            "opacity" if value.parse::<f32>().is_ok_and(|v| v <= 0.01) => return true,
            "position" => positioned = value == "absolute" || value == "fixed",
            "left" | "top" => offset |= far(OFF_PAGE_PX),
            "margin-left" | "margin-top" | "text-indent" => margin |= far(OFF_PAGE_PX * 10.0),
            _ => {}
        }
    }
    margin || (positioned && offset)
}

/// Supporting signal only: the address carries a long random-looking token, as a per-recipient
/// pixel does. Looked for in the query, and in path segments that are not picture files: a
/// hashed file name (`/a1b2…c3.png`) is a CDN's way of naming pictures, not a recipient.
pub fn has_token(url: &str) -> bool {
    let Some((_, path, query)) = parts(url) else { return false };
    let picture_file = |segment: &str| {
        let lower = segment.to_ascii_lowercase();
        [".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif", ".bmp", ".ico"].iter().any(|ext| lower.ends_with(ext))
    };
    query.split('&').filter_map(|pair| pair.split_once('=').map(|(_, v)| v).or(Some(pair))).any(random_looking)
        || path.split('/').filter(|s| !picture_file(s)).any(random_looking)
}

/// A run of URL-safe characters long enough, with letters and digits well mixed: what a token
/// generator writes, not a word or a date.
fn random_looking(text: &str) -> bool {
    if text.len() < TOKEN_MIN || text.len() > 2_048 {
        return false;
    }
    if !text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=' | b'%' | b'.' | b'~' | b'+')) {
        return false;
    }
    let digits = text.bytes().filter(u8::is_ascii_digit).count();
    let letters = text.bytes().filter(u8::is_ascii_alphabetic).count();
    // How often a letter is followed by a digit or the other way round: words and slugs rarely switch.
    let switches = text
        .as_bytes()
        .windows(2)
        .filter(|w| w[0].is_ascii_digit() != w[1].is_ascii_digit() && w[0].is_ascii_alphanumeric() && w[1].is_ascii_alphanumeric())
        .count();
    digits >= 4 && letters >= 4 && switches >= 4
}

/// Whether `img`, the `index`th of `all` pictures of a body, is a tracker.
pub fn is_tracker(all: &[Img], index: usize) -> bool {
    let Some(img) = all.get(index) else { return false };
    if known(&img.src) || unseen(img) {
        return true;
    }
    // A token is only ever a supporting signal: it takes the sole picture from its host, at the
    // end of the body, without a description, and of no size meant to be seen.
    if !has_token(&img.src) {
        return false;
    }
    let host = parts(&img.src).map(|(h, _, _)| h);
    let sole = host.is_some() && all.iter().filter(|other| parts(&other.src).map(|(h, _, _)| h) == host).count() == 1;
    let last = index + 1 == all.len();
    let (width, height) = declared(img);
    let seen = width.is_some_and(|w| w >= SEEN_PX) || height.is_some_and(|h| h >= SEEN_PX);
    sole && last && img.alt.trim().is_empty() && !seen
}

/// The trackers of a message's HTML: their addresses as the reader keeps them, each once, in the
/// order written, [`MAX_TRACKERS`] at most.
pub fn scan(html: &str) -> Vec<String> {
    let all = images(html);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for index in 0..all.len() {
        if out.len() >= MAX_TRACKERS {
            break;
        }
        if is_tracker(&all, index) && !all[index].src.is_empty() && seen.insert(all[index].src.clone()) {
            out.push(all[index].src.clone());
        }
    }
    out
}

/// The `<img>` tags of `html` that refer to the web, in order: within [`MAX_SCAN_BYTES`] and
/// [`MAX_IMAGES`]. A tolerant reading of the tag, as a browser's: quoted or bare values, any case.
pub fn images(html: &str) -> Vec<Img> {
    let mut end = html.len().min(MAX_SCAN_BYTES);
    while !html.is_char_boundary(end) {
        end -= 1;
    }
    let text = &html[..end];
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at + 4 < bytes.len() && out.len() < MAX_IMAGES {
        let Some(found) = find_img(bytes, at) else { break };
        let (img, next) = tag(text, found + 4);
        at = next.max(found + 4);
        if let Some(img) = img.filter(|img| parts(&img.src).is_some()) {
            out.push(img);
        }
    }
    out
}

/// Where the next `<img` that opens a tag is, from `from`.
fn find_img(bytes: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    while at + 4 < bytes.len() {
        if bytes[at] == b'<'
            && bytes[at + 1..at + 4].eq_ignore_ascii_case(b"img")
            && matches!(bytes[at + 4], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/' | b'>')
        {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// The attributes of a tag from `from` (just after its name) to its `>`; and where it ends.
fn tag(text: &str, from: usize) -> (Option<Img>, usize) {
    let bytes = text.as_bytes();
    let mut img = Img::default();
    let mut at = from;
    let mut src = None;
    loop {
        while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b'/') {
            at += 1;
        }
        if at >= bytes.len() {
            return (None, at);
        }
        if bytes[at] == b'>' {
            break;
        }
        let name_start = at;
        while at < bytes.len() && !bytes[at].is_ascii_whitespace() && !matches!(bytes[at], b'=' | b'>' | b'/') {
            at += 1;
        }
        let name = text[name_start..at].to_ascii_lowercase();
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut value = "";
        if at < bytes.len() && bytes[at] == b'=' {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            if at < bytes.len() && (bytes[at] == b'"' || bytes[at] == b'\'') {
                let quote = bytes[at];
                let start = at + 1;
                let close = bytes[start..].iter().position(|b| *b == quote).map_or(bytes.len(), |p| start + p);
                value = &text[start..close];
                at = (close + 1).min(bytes.len());
            } else {
                let start = at;
                while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>' {
                    at += 1;
                }
                value = &text[start..at];
            }
        }
        match name.as_str() {
            "src" if src.is_none() => src = Some(value),
            "width" => img.width = px(value),
            "height" => img.height = px(value),
            "style" => img.style = decode(value),
            "alt" => img.alt = decode(value),
            _ => {}
        }
    }
    img.src = src.map(|s| decode(s).chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect::<String>().trim().to_string()).unwrap_or_default();
    (Some(img), at + 1)
}

/// The few character references that matter in an address or a style.
fn decode(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    value.replace("&amp;", "&").replace("&#38;", "&").replace("&#x26;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'")
}
