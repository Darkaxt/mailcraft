// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender images: a picture in place of a sender's initials, in the message list, over the
//! message being read, and in place of a contact's in People.
//!
//! Off unless the owner turns it on in File › Options. Asking for a picture tells whoever keeps
//! it whose picture is looked at, so it is asked for only
//!
//! - for a sender the owner knows: one of the contacts, or somebody mail in Sent Items went to
//!   (in People, the contacts alone, whose addresses are the owner's own word: the rules below
//!   that are about a message don't apply there);
//! - or for mail whose pictures the owner lets come anyway: all mail (Download pictures in mail
//!   automatically), its sender's or its sender's domain's, or that message's
//!   ([`crate::reading::remote::access`]). Anybody can write to anybody, and a stranger's
//!   address, or a domain made for one recipient alone, learns when it is looked up: that is for
//!   the owner to allow, as the pictures in a stranger's mail are;
//! - only when the server that took the mail in vouches for its sender (DMARC,
//!   [`crate::reading::remote::vouched`]): anybody can write a `From:`, and a forged contact's
//!   face or a forged bank's logo would lend the forgery their trust. Mail from a server that
//!   writes no `Authentication-Results` keeps its initials;
//! - never for mail in Junk E-mail, for the same reason;
//! - only while a row of theirs is on screen, a few at a time, and once while the program runs.
//!
//! This crate opens no connections. It hands the host an address ([`Services::sender_images`])
//! and takes back bytes or nothing: the host asks Gravatar, then favicon.im, then Google,
//! through the same checked download and the same cache on disk as the pictures in mail. What
//! comes is a stranger's bytes, decoded within the same caps; an address that has no picture
//! keeps its initials.
//!
//! [`Services::sender_images`]: crate::Services::sender_images

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Rect, Ui, vec2};
use mailcraft_model::{Address, Message};
use serde_json::{Value, json};

use crate::MailApp;
use crate::reading::images;
use crate::theme::Tokens;

/// Most addresses whose picture, or lack of one, is kept in memory (a picture is at most
/// [`SIDE`] pixels a side, 64 KB): far more than a screen of rows shows, and those on screen
/// are never let go of for another.
const MAX_KEPT: usize = 300;
/// Most addresses the host looks up at a time.
const AT_A_TIME: usize = 4;
/// Most addresses waiting for their turn.
const MAX_WANTED: usize = 64;
/// Largest picture taken, in bytes: a sender's picture is small.
const MAX_BYTES: usize = 1024 * 1024;
/// Longest side a picture is kept at, in pixels.
const SIDE: u32 = 128;
/// Longest side of a picture taken, in pixels: it is decoded whole on the window's thread.
const MAX_SIDE: u32 = 1024;
/// How long the host is waited on for one address, in milliseconds: it may try three places,
/// each for up to 30 s (`mailcraft_net::pictures::PATIENCE`).
const PATIENCE_MS: f64 = 100_000.0;
/// Longest address looked up, as the host's: a longer one (a contact's from a file can be a
/// megabyte) is neither lowered nor kept.
const MAX_ADDRESS: usize = 320;

/// What is known of one address's picture.
enum Look {
    /// The host is looking (since when, in milliseconds).
    Asked(Receiver<Result<Vec<u8>, String>>, f64),
    /// Its bytes, not decoded yet: one picture is decoded in a frame.
    Came(Vec<u8>),
    /// Decoded; `true`: a site's icon of one tone ([`site_look`]), painted in the text's colour.
    Ready(egui::TextureHandle, bool),
    /// There is none to be had: the initials stay.
    // shortcut: not asked about again until the program restarts, even when the network was down.
    Missing,
}

/// Senders' pictures, for as long as the program runs.
#[derive(Default)]
pub struct SenderImages {
    /// By address in lower case, with the frame it was last on screen in.
    looks: HashMap<String, (Look, u64)>,
    /// Addresses on screen that nothing is known of yet: asked for at the next frame.
    wanted: Vec<String>,
    /// The frame arrivals were last looked for in, and the frame a picture was last decoded in.
    ticked: Option<u64>,
    decoded: Option<u64>,
    /// How many look-ups the host had in hand at the last frame.
    flying: usize,
    /// Handed to the host with every look-up, weakly: gone with this state, it stops what is in
    /// flight before its next request.
    alive: Arc<()>,
}

impl SenderImages {
    /// Let go of the senders' pictures kept in memory (the window is out of sight,
    /// [`crate::out_of_sight`]): they are asked for again (from the host's cache first) when a
    /// row of theirs is next on screen. What is known to have no picture is kept, and is not
    /// asked about again; a look-up on its way is taken in when rows are next shown.
    pub(crate) fn let_go_of_pictures(&mut self) {
        self.looks.retain(|_, (look, _)| matches!(look, Look::Missing | Look::Asked(..)));
        self.decoded = None;
    }
}

/// Whether the owner knows `address` (in lower case): it is one of the contacts', or mail in
/// Sent Items went to it.
fn is_known(app: &mut MailApp, address: &str) -> bool {
    app.session.knows(address)
}

/// Take in what the host found, and hand it the addresses rows wanted a frame ago. Once a frame.
fn tick(app: &mut MailApp, ctx: &egui::Context) {
    let frame = images::frame(ctx);
    let offline = app.offline();
    let state = &mut app.sender_images;
    if state.ticked == Some(frame) {
        return;
    }
    state.ticked = Some(frame);
    let now = crate::now_ms();
    let mut flying = 0;
    for (look, _) in state.looks.values_mut() {
        let Look::Asked(answer, since) = look else { continue };
        *look = match answer.try_recv() {
            // A stranger's bytes: a picture within the caps, or none.
            Ok(Ok(bytes)) if bytes.len() <= MAX_BYTES && images::probe_icon(&bytes).is_some_and(|[w, h]| w.max(h) <= MAX_SIDE) => Look::Came(bytes),
            Ok(_) | Err(TryRecvError::Disconnected) => Look::Missing,
            Err(TryRecvError::Empty) if now - *since > PATIENCE_MS => Look::Missing,
            Err(TryRecvError::Empty) => {
                flying += 1;
                continue;
            }
        };
        ctx.request_repaint();
    }
    // Taken every frame, offline too: back online, only what is on screen then is asked for.
    let wanted = std::mem::take(&mut state.wanted);
    // (Working offline, nobody is asked: senders keep their initials.)
    if !offline && let Some(look_up) = &app.services.sender_images {
        // The rest is wanted again by its row, for as long as that is on screen.
        for address in wanted.into_iter().take(AT_A_TIME.saturating_sub(flying)) {
            if state.looks.contains_key(&address) {
                continue;
            }
            trim(&mut state.looks, frame);
            let answer = look_up(address.clone(), Arc::downgrade(&state.alive));
            state.looks.insert(address, (Look::Asked(answer, now), frame));
            flying += 1;
        }
    }
    state.flying = flying;
    if flying > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Forget what was on screen longest ago while there is too much; never what is on its way.
fn trim(looks: &mut HashMap<String, (Look, u64)>, frame: u64) {
    while looks.len() >= MAX_KEPT {
        let oldest = looks
            .iter()
            .filter(|(_, (look, used))| *used < frame && !matches!(look, Look::Asked(..)))
            .min_by_key(|(_, (_, used))| *used)
            .map(|(address, _)| address.clone());
        match oldest {
            Some(address) => drop(looks.remove(&address)),
            None => return,
        }
    }
}

/// The picture to show for `who`, on the mail `m`, in place of their initials. `None` while
/// there is none; one that is still to be looked up is noted, and asked for at the next frame.
pub fn picture(app: &mut MailApp, ctx: &egui::Context, m: &Message, who: &Address) -> Option<egui::TextureHandle> {
    if !app.ui.sender_images || app.services.sender_images.is_none() {
        return None;
    }
    if app.session.mailbox.in_junk(m.folder) {
        return None;
    }
    tick(app, ctx);
    let address = key(who)?;
    // The say-so for a message's pictures is its sender's: whoever it goes to must be known.
    let sender = key(&m.from).is_some_and(|from| from == address);
    // Who it went to is the owner's word (it is theirs); who sent it, only the server's.
    if sender && !crate::reading::remote::vouched(app.session.mailbox.account(m.account), m) {
        return None;
    }
    if !is_known(app, &address) && !(sender && crate::reading::remote::access(app, m) == crate::reading::Access::Allowed) {
        return None;
    }
    look(app, ctx, address).map(|(texture, _)| texture)
}

/// The picture to show for one of the contacts in People, in place of their initials. Their
/// address is the owner's own word, as whoever the owner's mail went to is: no server's is needed.
pub fn contact_picture(app: &mut MailApp, ctx: &egui::Context, who: &Address) -> Option<egui::TextureHandle> {
    if !app.ui.sender_images || app.services.sender_images.is_none() {
        return None;
    }
    tick(app, ctx);
    let address = key(who)?;
    if !is_known(app, &address) {
        return None;
    }
    look(app, ctx, address).map(|(texture, _)| texture)
}

/// The icon of the site folder `id`'s mail comes from (`mailcraft_model::folder_sites`), while
/// the owner keeps folder icons on. `None` while there is none; one still to be looked up is
/// noted, and asked for at the next frame. The site is asked first, as the host does it.
pub fn folder_icon(app: &mut MailApp, ctx: &egui::Context, id: mailcraft_model::Id) -> Option<SiteIcon> {
    if !app.ui.folder_icons || app.services.sender_images.is_none() {
        return None;
    }
    let site = app.session.folder_site(id)?;
    tick(app, ctx);
    let (texture, mono) = look(app, ctx, format!("{}{site}", mailcraft_model::folder_sites::ICON_KEY))?;
    Some(SiteIcon { texture, mono })
}

/// A site's icon, as it is shown before a folder's name: on the list itself, as an emoji is, with
/// no tile behind it (v0.12.38).
pub struct SiteIcon {
    pub texture: egui::TextureHandle,
    /// Of one tone (a black or a white logo): painted in the text's colour, so that it reads in
    /// either theme.
    pub mono: bool,
}

impl SiteIcon {
    /// What to tint it with: the text's colour for one of one tone, else nothing.
    pub fn tint(&self, text: egui::Color32) -> egui::Color32 {
        if self.mono { text } else { egui::Color32::WHITE }
    }
}

/// Makes a site's icon fit to be shown without a tile behind it (v0.12.38), and says whether it is
/// of one tone. A white margin round it, which would show as a white square, is made see-through
/// (the near-white reached from its edge, softly at the borderline); one of one dark or one light
/// tone (a black or a white logo, with nothing of the other) becomes white with its shape, for
/// [`SiteIcon::tint`] to paint in the text's colour.
pub(crate) fn site_look(image: &mut egui::ColorImage) -> bool {
    let [w, h] = image.size;
    if w == 0 || h == 0 {
        return false;
    }
    let mut px: Vec<[u8; 4]> = image.pixels.iter().map(|c| c.to_srgba_unmultiplied()).collect();
    let white = |p: [u8; 4]| p[3] >= 224 && p[0].min(p[1]).min(p[2]) >= 232;
    let edge: Vec<usize> = (0..w).flat_map(|x| [x, (h - 1) * w + x]).chain((0..h).flat_map(|y| [y * w, y * w + w - 1])).collect();
    if edge.iter().filter(|&&i| px.get(i).is_some_and(|&p| white(p))).count() * 10 >= edge.len() * 7 {
        let pale = |p: [u8; 4]| p[3] > 0 && p[0].min(p[1]).min(p[2]) >= 216;
        let mut seen = vec![false; w * h];
        let mut todo: Vec<usize> = Vec::new();
        for &i in &edge {
            if px.get(i).is_some_and(|&p| pale(p)) && !seen[i] {
                seen[i] = true;
                todo.push(i);
            }
        }
        while let Some(i) = todo.pop() {
            let p = px[i];
            let least = p[0].min(p[1]).min(p[2]);
            // White goes; the near-white at its border fades out, as an edge drawn smooth does.
            px[i][3] = if least >= 236 { 0 } else { u8::try_from(u32::from(p[3]) * u32::from(236 - least) / 20).unwrap_or(p[3]) };
            let (x, y) = (i % w, i / w);
            let next = [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)];
            for n in next.into_iter().flatten() {
                if !seen[n] && pale(px[n]) {
                    seen[n] = true;
                    todo.push(n);
                }
            }
        }
    }
    // Of one tone: what is drawn is grey, all of it dark or all of it light.
    let ink: Vec<[u8; 4]> = px.iter().copied().filter(|p| p[3] >= 128).collect();
    let luma = |p: &[u8; 4]| (2126 * u32::from(p[0]) + 7152 * u32::from(p[1]) + 722 * u32::from(p[2])) / 10_000;
    let n = ink.len() as u64;
    let grey = n > 0 && ink.iter().map(|p| u64::from(p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))).sum::<u64>() < n * 31;
    let dark = ink.iter().filter(|p| luma(p) <= 100).count() as u64;
    let light = ink.iter().filter(|p| luma(p) >= 150).count() as u64;
    let mono = grey && (dark * 100 >= n * 97 || light * 100 >= n * 97);
    image.pixels = px
        .iter()
        .map(|p| {
            if mono {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, p[3])
            } else {
                egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])
            }
        })
        .collect();
    mono
}

/// `who`'s address in lower case, as pictures are kept by; none for one too long to be anybody's.
fn key(who: &Address) -> Option<String> {
    let email = who.email.trim();
    (email.len() <= MAX_ADDRESS).then(|| email.to_lowercase())
}

/// The picture of `address` (in lower case), once its look-up has brought one; one that is still
/// to be looked up is noted, and asked for at the next frame.
fn look(app: &mut MailApp, ctx: &egui::Context, address: String) -> Option<(egui::TextureHandle, bool)> {
    let frame = images::frame(ctx);
    let offline = app.offline();
    let state = &mut app.sender_images;
    let Some((look, used)) = state.looks.get_mut(&address) else {
        // Working offline nobody is asked, so nothing waits for its turn or repaints for it.
        if !offline && state.wanted.len() < MAX_WANTED && !state.wanted.contains(&address) {
            state.wanted.push(address);
            // With every place taken, the 250 ms repaint of `tick` comes back for it.
            if state.flying < AT_A_TIME {
                ctx.request_repaint();
            }
        }
        return None;
    };
    *used = frame;
    if let Look::Came(bytes) = look {
        if state.decoded == Some(frame) {
            ctx.request_repaint();
            return None;
        }
        state.decoded = Some(frame);
        *look = match images::decode_icon(bytes, SIDE) {
            Some(mut image) => {
                let mono = address.starts_with(mailcraft_model::folder_sites::ICON_KEY) && site_look(&mut image);
                Look::Ready(ctx.load_texture("mailcraft-sender-image", image, egui::TextureOptions::LINEAR), mono)
            }
            None => Look::Missing,
        };
    }
    match look {
        Look::Ready(texture, mono) => Some((texture.clone(), *mono)),
        _ => None,
    }
}

/// `who`'s round avatar on the mail `m`: their picture when there is one, else their initials.
pub fn avatar(app: &mut MailApp, ui: &Ui, center: egui::Pos2, radius: f32, m: &Message, who: &Address) {
    let texture = picture(app, ui.ctx(), m, who);
    paint(ui, center, radius, texture, who);
}

/// A contact's round avatar in People: their picture when there is one, else their initials.
pub fn contact_avatar(app: &mut MailApp, ui: &Ui, center: egui::Pos2, radius: f32, who: &Address) {
    let texture = contact_picture(app, ui.ctx(), who);
    paint(ui, center, radius, texture, who);
}

fn paint(ui: &Ui, center: egui::Pos2, radius: f32, texture: Option<egui::TextureHandle>, who: &Address) {
    let Some(texture) = texture else {
        crate::widgets::avatar(ui, center, radius, who);
        return;
    };
    // On white, as a browser shows an icon: one with see-through parts stays readable in the dark.
    ui.painter().circle_filled(center, radius, crate::theme::Tokens::get(ui.ctx()).avatar_backdrop);
    egui::Image::from_texture(egui::load::SizedTexture::from_handle(&texture))
        .corner_radius(radius)
        .paint_at(ui, Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0)));
}

/// The row of File › Options, saying who is asked and what each is told.
pub fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let mut on = app.ui.sender_images;
    if ui.checkbox(&mut on, tl!("Show sender images from Gravatar, favicon.im and Google")).changed() {
        let _ = app.run("ui.senderImages", json!({"value": on}));
    }
    let said = tl!(
        "Gravatar is asked for the sender's picture and told a hash of their address; else favicon.im, then Google, are asked for their website's icon and told the sender's domain. This tells those services whose pictures you look at. Only for the people you know (your contacts, and whoever you have written to) and the mail whose pictures you download, never for Junk E-mail, and a sender's picture only when your mail server confirms the mail is from them."
    );
    ui.label(egui::RichText::new(said).color(t.text_dim));
    let mut on = app.ui.folder_icons;
    if ui.checkbox(&mut on, tl!("Show folder icons")).changed() {
        let _ = app.run("ui.folderIcons", json!({"value": on}));
    }
    let said =
        tl!("A folder whose mail is nearly all from one company shows its website's icon. The website is asked first, then favicon.im and Google.");
    ui.label(egui::RichText::new(said).color(t.text_dim));
}

/// `ui.senderImages`, forgetting what is in memory when it is turned off; `ui.folderIcons`.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id == "ui.folderIcons" {
        let on = p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.folder_icons);
        app.ui.folder_icons = on;
        return Some(Ok(json!({"value": on})));
    }
    (id == "ui.senderImages").then(|| {
        let on = p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.sender_images);
        app.ui.sender_images = on;
        if !on {
            app.sender_images = SenderImages::default();
        }
        Ok(json!({"value": on}))
    })
}

#[cfg(test)]
#[path = "sender_images_tests.rs"]
mod tests;
