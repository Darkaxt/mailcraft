// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A sender's picture for the message list and a contact's for People, when the owner turned
//! that on in File › Options:
//! Gravatar's for the address, else the icon favicon.im has for its domain, else Google's.
//!
//! The window asks only about senders the owner knows or whose mail's pictures the owner lets
//! come (in People, the contacts alone), never about mail in Junk E-mail (`mailcraft_ui_egui::sender_images`). Here an address
//! becomes the places to ask, and each is downloaded and kept like a message's pictures
//! ([`picture_cache::picture`]): checked on the way, capped, and kept on disk so that it is not
//! asked for again at the next start.
//!
//! - **Gravatar** is told the SHA-256 of the address in lower case (its way of naming one), not
//!   the address, and answers 404 rather than a stand-in picture when it has none (`d=404`).
//! - **favicon.im** is told the domain of the sender's site alone ([`site`]: `nutrient.io` for
//!   `mail.nutrient.io`, which has no icon of its own), and answers 404 rather than a stand-in
//!   icon when it has none (`throw-error-on-404`). Not for a mail provider's own domain
//!   (`gmail.com`), whose icon says nothing of the sender, nor for a name on a hosting platform
//!   (`contoso.onmicrosoft.com`), which would wear the platform's icon.
//! - **Google's favicon service**, only when favicon.im had none or none that is shown (an SVG):
//!   it is told the same domain, and sends every icon as PNG. It has no documented terms, so it
//!   comes last; when it has no icon it answers 404 with a globe of its own, which, like any
//!   answer that isn't 200, is never shown.

use std::sync::{Arc, Weak};

use mailcraft_engine::store::Digest;
use mailcraft_model::{folder_sites, provider};

use crate::picture_cache::{self, Cache};

/// Longest address looked up.
const MAX_ADDRESS: usize = 320;
/// The size asked of Gravatar and Google, in pixels: a list's avatar on a screen of three times
/// the density.
const SIZE: u32 = 96;

/// Where a picture of `address` may be had, in the order they are asked; none for what is no
/// address.
pub fn addresses(address: &str) -> Vec<String> {
    let address = address.trim().to_lowercase();
    let plain = address.len() <= MAX_ADDRESS && !address.chars().any(|c| c.is_whitespace() || c.is_control());
    let Some((_, domain)) = address.rsplit_once('@').filter(|(name, _)| plain && !name.is_empty()) else { return Vec::new() };
    if !provider::is_domain(domain) {
        return Vec::new();
    }
    let mut urls = vec![format!("https://gravatar.com/avatar/{}?s={SIZE}&d=404", Digest::of(address.as_bytes()).hex())];
    let site = site(domain);
    // A customer's name on a hosting platform would wear the platform's icon, as a phish does.
    if !provider::is_mail_provider(site) && !provider::is_public_suffix(site) && !provider::on_hosting_platform(site) {
        urls.push(format!("https://favicon.im/{site}?larger=true&throw-error-on-404=true"));
        urls.push(format!("https://www.google.com/s2/favicons?domain={site}&sz={SIZE}"));
    }
    urls
}

/// The site a mail domain is part of ([`provider::site`]). A name made up for one recipient alone
/// (`u8f3a.spam.example`) is not told to anybody either.
fn site(domain: &str) -> &str {
    provider::site(domain)
}

/// Whether bytes begin as a sender's picture does: as a message's pictures do
/// ([`picture_cache::looks_like_a_picture`]), or as an ICO holding a picture or more, a web
/// site's icon, which only a sender's picture may be.
fn looks_like_a_picture(bytes: &[u8]) -> bool {
    picture_cache::looks_like_a_picture(bytes) || (bytes.starts_with(b"\0\0\x01\0") && bytes.get(4..6).is_some_and(|count| count != b"\0\0"))
}

/// The picture of `address`: from the first of [`addresses`] that has one, downloaded with
/// `fetch` (`mailcraft_net::pictures::fetch`), for as long as the window still wants it
/// (`wanted` can be upgraded). Once it doesn't, nothing more is asked: a request already on its
/// way goes on, and what it brings is not kept. On a background thread: it waits for the network.
pub fn look_up<E: std::fmt::Display>(
    address: &str,
    cache: Option<&Arc<Cache>>,
    wanted: &Weak<()>,
    fetch: impl Fn(&str) -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, String> {
    let turned_off = || Err("sender images were turned off".to_string());
    if wanted.strong_count() == 0 {
        return turned_off();
    }
    if let Some(site) = address.strip_prefix(folder_sites::ICON_KEY) {
        return site_icon(site, cache, wanted, &fetch);
    }
    let urls = addresses(address);
    // What is kept, from any of them, before anything is asked: a place that had nothing is not
    // asked again at each start when a later one's picture is kept.
    let kept = urls.iter().find_map(|url| cache.and_then(|cache| cache.get_as(url, looks_like_a_picture)));
    if let Some(bytes) = kept {
        return Ok(bytes);
    }
    let mut why = "no picture is looked up for this address".to_string();
    for url in urls {
        if wanted.strong_count() == 0 {
            return turned_off();
        }
        match fetch(&url) {
            Ok(bytes) if looks_like_a_picture(&bytes) => {
                // Not kept when the window let go while it came (turned off, or cleared).
                if wanted.strong_count() == 0 {
                    return turned_off();
                }
                if let Some(cache) = cache {
                    cache.put_as(&url, &bytes, looks_like_a_picture);
                }
                return Ok(bytes);
            }
            // An icon in a kind that isn't shown (an SVG): the initials stay.
            Ok(_) => why = "what came is not a picture that is shown".to_string(),
            Err(e) => why = e.to_string(),
        }
    }
    Err(why)
}

/// Where a folder's icon is kept once found, whichever place it came from: the site's own is
/// asked first, and a page of its is not read again at each start (kept [`picture_cache::MAX_AGE`]).
fn site_icon_key(site: &str) -> String {
    format!("https://{site}/#mailcraft-folder-icon")
}

/// The icon of `site` for a folder (`mailcraft_model::folder_sites`): the site's own first (the
/// icons its home page names, the largest first, then its `/favicon.ico`), then favicon.im's,
/// then Google's. The site is the owner's correspondent, whom the folder is for: asking it tells
/// nobody new; the two services come only after.
fn site_icon<E: std::fmt::Display>(
    site: &str,
    cache: Option<&Arc<Cache>>,
    wanted: &Weak<()>,
    fetch: &impl Fn(&str) -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, String> {
    let site = site.trim().to_lowercase();
    if site.len() > MAX_ADDRESS || !folder_sites::wears_an_icon(&site) || provider::site(&site) != site {
        return Err("no icon is looked up for this site".to_string());
    }
    let key = site_icon_key(&site);
    if let Some(bytes) = cache.and_then(|cache| cache.get_as(&key, looks_like_a_picture)) {
        return Ok(bytes);
    }
    let turned_off = || Err("folder icons were turned off".to_string());
    let mut urls = match fetch(&format!("https://{site}/")) {
        Ok(page) => icon_links(&page, &site),
        Err(_) => Vec::new(),
    };
    urls.push(format!("https://{site}/favicon.ico"));
    urls.push(format!("https://favicon.im/{site}?larger=true&throw-error-on-404=true"));
    urls.push(format!("https://www.google.com/s2/favicons?domain={site}&sz={SIZE}"));
    let mut why = "the site has no icon".to_string();
    for url in urls {
        if wanted.strong_count() == 0 {
            return turned_off();
        }
        match fetch(&url) {
            Ok(bytes) if looks_like_a_picture(&bytes) => {
                if wanted.strong_count() == 0 {
                    return turned_off();
                }
                if let Some(cache) = cache {
                    cache.put_as(&key, &bytes, looks_like_a_picture);
                }
                return Ok(bytes);
            }
            Ok(_) => why = "what came is not a picture that is shown".to_string(),
            Err(e) => why = e.to_string(),
        }
    }
    Err(why)
}

/// Most of a home page read for its icons: they are named in its `<head>`.
const MAX_PAGE: usize = 512 * 1024;
/// Most icons a page names that are tried.
const MAX_LINKS: usize = 4;

/// The icons a home page of `site` names (`<link rel="icon">`, `apple-touch-icon`), as `https://`
/// addresses, the largest first (up to 256 pixels; larger ones after, smallest of those first).
/// Not an SVG (not shown), nor one written into the page (`data:`).
fn icon_links(page: &[u8], site: &str) -> Vec<String> {
    let page = String::from_utf8_lossy(&page[..page.len().min(MAX_PAGE)]);
    // (ASCII lower case keeps every byte where it was.)
    let lower = page.to_ascii_lowercase();
    let mut found: Vec<(u32, String)> = Vec::new();
    let mut at = 0;
    while let Some(start) = lower[at..].find("<link").map(|i| at + i) {
        let end = lower[start..].find('>').map_or(lower.len(), |i| start + i);
        at = end;
        let attrs = attributes(&page[start + 5..end]);
        let get = |name: &str| attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
        let rel = get("rel").unwrap_or("").to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_ascii_whitespace().collect();
        let touch = rels.iter().any(|r| r.starts_with("apple-touch-icon"));
        if !(touch || rels.contains(&"icon")) {
            continue;
        }
        let Some(href) = get("href").map(|h| h.trim().replace("&amp;", "&")).filter(|h| !h.is_empty()) else { continue };
        let svg = get("type").is_some_and(|t| t.to_ascii_lowercase().contains("svg"))
            || href.to_ascii_lowercase().split(['?', '#']).next().is_some_and(|p| p.ends_with(".svg"));
        let Some(url) = absolute(&href, site).filter(|_| !svg) else { continue };
        let side = get("sizes").and_then(|s| s.split_ascii_whitespace().filter_map(|s| s.split_once(['x', 'X'])?.0.parse::<u32>().ok()).max());
        let side = side.unwrap_or(if touch { 180 } else { 16 });
        if !found.iter().any(|(_, u)| *u == url) {
            found.push((side, url));
        }
    }
    found.sort_by_key(|(side, _)| if *side <= 256 { (0, u32::MAX - side) } else { (1, *side) });
    found.into_iter().map(|(_, url)| url).take(MAX_LINKS).collect()
}

/// The attributes of a tag (what follows its name), names in lower case.
fn attributes(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = tag;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let name_end = rest.find(|c: char| c.is_whitespace() || c == '=' || c == '/').unwrap_or(rest.len());
        if name_end == 0 {
            return out;
        }
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let value = match rest.strip_prefix('=').map(str::trim_start) {
            Some(after) if after.starts_with(['"', '\'']) => {
                let quote = &after[..1];
                let body = &after[1..];
                let close = body.find(quote).unwrap_or(body.len());
                rest = body.get(close + 1..).unwrap_or("");
                body[..close].to_string()
            }
            Some(after) => {
                let close = after.find(char::is_whitespace).unwrap_or(after.len());
                rest = &after[close..];
                after[..close].to_string()
            }
            None => String::new(),
        };
        out.push((name, value));
    }
}

/// `href` on the home page of `site`, as an `https://` address; none for any other scheme.
fn absolute(href: &str, site: &str) -> Option<String> {
    let lower = href.to_ascii_lowercase();
    let scheme = lower.find(':').filter(|colon| lower.find(['/', '?', '#']).is_none_or(|slash| *colon < slash));
    let url = if lower.starts_with("https://") {
        href.to_string()
    } else if lower.starts_with("http://") {
        format!("https://{}", &href[7..])
    } else if scheme.is_some() {
        // `data:`, `javascript:` and the like.
        return None;
    } else if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else if let Some(path) = href.strip_prefix('/') {
        format!("https://{site}/{path}")
    } else {
        format!("https://{site}/{href}")
    };
    (url.len() <= MAX_ADDRESS && !url.chars().any(|c| c.is_whitespace() || c.is_control())).then_some(url)
}

#[cfg(test)]
#[path = "sender_images_tests.rs"]
mod tests;
