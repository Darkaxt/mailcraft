// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! In-place, selectable source. Only bounded pages live in the UI; the engine
//! owns preparation, integrity checking and the lifetime of native snapshots.
//!
//! The source reads as one continuous text: pages of [`PAGE`] bytes are asked for one after
//! another as the owner scrolls towards the end of what is here, up to [`MAX_LOADED`] bytes
//! (past that a note says where it stops; Export .eml has all of it). A page ends at a line's
//! end (what is left of the line goes to the next page), so that each can be coloured on its own:
//! header names and values, MIME boundaries, HTML tags, attributes, strings and comments, and
//! encoded blocks (base64, quoted-printable) dimmed ([`spans`]). The colouring is worked out once
//! per page as it comes; a page is laid out only while it is in view, and kept laid out while it is.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Galley, Rect, Ui, pos2, vec2};
use mailcraft_model::Message;
use serde_json::{Value, json};

use crate::{MailApp, theme::Tokens};

const PAGE: usize = 32 << 10;
/// Most bytes of source shown in place.
pub(crate) const MAX_LOADED: u64 = 16 << 20;
/// Size of the fixed-width text.
const FONT_SIZE: f32 = 12.0;
/// How far below what is in view the next page is asked for, in screens.
const AHEAD: f32 = 1.5;

#[derive(Default)]
pub(crate) struct Pane {
    active: Option<Active>,
    retiring: Vec<(u64, &'static str, u64)>,
}

struct Active {
    id: u64,
    opening: u64,
    /// The version the source was read from; replaced by flag or sync updates.
    message: Arc<Message>,
    body: Reader,
    export: Option<Reader>,
}

struct Reader {
    command: &'static str,
    token: Option<u64>,
    offset: u64,
    /// The source's pages that are here, in order (the body reader's only).
    pages: Vec<Page>,
    /// The start of a line that the last page cut: it begins the next one.
    carry: String,
    answer: Option<Value>,
    wanted: bool,
    downloading: bool,
    error: Option<String>,
}

impl Reader {
    fn new(command: &'static str) -> Self {
        Self { command, token: None, offset: 0, pages: Vec::new(), carry: String::new(), answer: None, wanted: true, downloading: false, error: None }
    }

    /// Bytes of source here.
    fn loaded(&self) -> u64 {
        self.pages.last().map_or(0, |page| page.next.unwrap_or(page.offset + page.text.len() as u64))
    }

    /// The offset of the page after the last one here, if there is one and it may be shown.
    fn next(&self) -> Option<u64> {
        self.pages.last().and_then(|page| page.next).filter(|next| *next < MAX_LOADED)
    }

    /// The source stops at [`MAX_LOADED`] though there is more.
    fn capped(&self) -> bool {
        self.pages.last().and_then(|page| page.next).is_some_and(|next| next >= MAX_LOADED)
    }

    /// A page came: added after the others, cut at its last line's end.
    fn take(&mut self, answer: &Value) {
        let text = answer["source"].as_str().unwrap_or_default();
        let next = answer["nextOffset"].as_u64();
        if self.offset == 0 || self.pages.last().and_then(|page| page.next) != Some(self.offset) {
            self.pages.clear();
            self.carry.clear();
        }
        let carried = self.carry.len();
        let mut whole = std::mem::take(&mut self.carry);
        whole.push_str(text);
        if next.is_some()
            && let Some(end) = whole.rfind('\n')
            && end + 1 < whole.len()
        {
            self.carry = whole[end + 1..].to_string();
            whole.truncate(end + 1);
        }
        let start = self.pages.last().map(|page| page.end).unwrap_or_default();
        let mut end = start;
        let runs = spans(&whole, &mut end);
        let lines = whole.split('\n').count().max(1);
        let columns = whole.split('\n').map(|line| line.chars().count()).max().unwrap_or(0);
        self.pages.push(Page { offset: self.offset, next, text: Arc::from(whole), carried, runs, end, lines, columns, laid: None });
    }

    /// "Previous": the last page goes, and what it carried comes back.
    fn drop_last(&mut self) -> Option<u64> {
        if self.pages.len() < 2 {
            return None;
        }
        let page = self.pages.pop()?;
        self.carry = page.text.get(..page.carried).unwrap_or_default().to_string();
        self.offset = self.pages.last().map_or(0, |page| page.offset);
        Some(self.offset)
    }
}

/// One page of source as it is shown.
struct Page {
    offset: u64,
    /// Where the next page begins; `None` for the last.
    next: Option<u64>,
    text: Arc<str>,
    /// How many bytes at its start came from the page before (a line it cut).
    carried: usize,
    /// What each run of the text is ([`spans`]).
    runs: Vec<(usize, Kind)>,
    /// Where the colouring stood at its end: the next page goes on from there.
    end: Scan,
    lines: usize,
    columns: usize,
    /// Laid out, while in view: for the colours and the display scale it was laid out for.
    laid: Option<(LaidKey, Arc<Galley>)>,
}

/// What a page's layout depends on besides its text.
#[derive(Clone, Copy, PartialEq)]
struct LaidKey {
    palette: [Color32; 4],
    scale: f32,
}

impl Reader {
    fn step(&mut self, app: &mut MailApp, id: u64) {
        if !self.wanted || self.error.is_some() {
            return;
        }
        if self.downloading {
            if app.session.mailbox.message(id).is_some_and(|m| m.unfetched) {
                if !app.fetching.busy() {
                    self.error = Some(tl!("The complete message could not be downloaded. Check Send/Receive before trying again.").into());
                }
                return;
            }
            self.downloading = false;
        }
        // One preparation worker serves both body and export. Waiting for it is
        // different from replaying a request whose token is already known.
        if self.token.is_none() && app.session.source_export_busy() {
            return;
        }
        let params = match self.token {
            Some(token) => json!({"id":id,"token":token,"offset":self.offset,"limit":PAGE}),
            None => json!({"id":id,"limit":PAGE}),
        };
        match app.execute(self.command, params) {
            Err(why) => self.error = Some(why),
            Ok(value) => {
                if value.get("pending").and_then(Value::as_str) == Some("download") {
                    self.downloading = true;
                    return;
                }
                if let Some(token) = value.get("token").and_then(Value::as_u64) {
                    self.token = Some(token);
                }
                match value.get("state").and_then(Value::as_str) {
                    Some("pending") => (),
                    Some("ready") => {
                        if self.command == "mail.sourcePage" {
                            self.take(&value);
                        }
                        self.answer = Some(value);
                        self.wanted = false;
                    }
                    _ => self.error = Some(tl!("The source reader is closed. Switch to Message and open Source again.").into()),
                }
            }
        }
    }
}

impl Pane {
    pub(crate) fn showing(&self, id: u64) -> bool {
        self.active.as_ref().is_some_and(|active| active.id == id)
    }

    fn retire(&mut self, active: Active) {
        for reader in std::iter::once(active.body).chain(active.export) {
            if let Some(token) = reader.token {
                self.retiring.push((active.id, reader.command, token));
            }
        }
    }

    pub(crate) fn inspect(&self) -> Value {
        self.active.as_ref().map_or(Value::Null, |active| {
            json!({
                "id":active.id,"mode":"source","offset":active.body.offset,
                "pages":active.body.pages.len(),"loaded":active.body.loaded(),"capped":active.body.capped(),
                "state":if active.body.error.is_some(){"failed"}else if active.body.wanted{"pending"}else{"ready"},
                "format":active.body.answer.as_ref().and_then(|v|v.get("format")),
                "nextOffset":active.body.answer.as_ref().and_then(|v|v.get("nextOffset")),
                "bytes":active.body.answer.as_ref().and_then(|v|v.get("bytes")),
                "exporting":active.export.is_some(),"error":active.body.error
            })
        })
    }
}

/// App-owned command, usable by the authenticated UI bridge without raw input.
/// Headless callers use mail.sourcePage/mail.exportEml instead.
pub(crate) fn command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, params);
        Err("The in-place source reader requires the native app. View Source remains available here.".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let object = params.as_object().ok_or("expected source options")?;
        if object.keys().any(|key| !matches!(key.as_str(), "on" | "page" | "export")) {
            return Err("unknown source option".into());
        }
        let on = match params.get("on") {
            Some(value) => Some(value.as_bool().ok_or("on must be a boolean")?),
            None => None,
        };
        let export = match params.get("export") {
            Some(value) => value.as_bool().ok_or("export must be a boolean")?,
            None => false,
        };
        let page = match params.get("page") {
            Some(value) => Some(value.as_str().filter(|page| matches!(*page, "next" | "previous")).ok_or("page must be next or previous")?),
            None => None,
        };
        if on == Some(false) && (export || page.is_some()) {
            return Err("open Source before paging or exporting".into());
        }
        let id = app.session.sel.focus().filter(|_| app.session.sel.messages.len() == 1).ok_or("select one message first")?;
        if on == Some(false) {
            if let Some(active) = app.source_pane.active.take() {
                app.source_pane.retire(active);
            }
        } else if !app.source_pane.showing(id) {
            if let Some(active) = app.source_pane.active.take() {
                app.source_pane.retire(active);
            }
            let message = app.session.mailbox.messages.get(&id).ok_or("the message no longer exists")?;
            app.source_pane.active =
                Some(Active { id, opening: app.session.openings(), message: message.clone(), body: Reader::new("mail.sourcePage"), export: None });
        }
        if let Some(active) = &mut app.source_pane.active {
            if let Some(page) = page {
                if active.body.wanted || active.body.error.is_some() {
                    return Err("wait for the current source page".into());
                }
                // The source scrolls on by itself; these are for callers without a scroll bar.
                if page == "next" {
                    if active.body.capped() {
                        return Err(format!("the source is shown up to {} MB; Export .eml has all of it", MAX_LOADED >> 20));
                    }
                    active.body.offset = active.body.next().ok_or("this is the last source page")?;
                    active.body.wanted = true;
                } else {
                    active.body.drop_last().ok_or("this is the first source page")?;
                }
            }
            if export {
                if active.export.is_some() || app.file_dialog.is_some() {
                    return Err("an export or file dialog is already open".into());
                }
                if app.services.file_dialog.is_none() {
                    return Err("use mail.exportEml for a programmatic export".into());
                }
                active.export = Some(Reader::new("mail.exportEml"));
            }
        }
        Ok(app.source_pane.inspect())
    }
}

/// Whether two versions of a message read as the same source: body, attachments and headers.
fn same_source(a: &Message, b: &Message) -> bool {
    a.id == b.id && a.unfetched == b.unfetched && a.headers == b.headers && a.original == b.original && crate::reading::same_body(a, b)
}

/// Poll actual producer/download results, including cancellation through exit.
pub(crate) fn tick(app: &mut MailApp) {
    let mut pane = std::mem::take(&mut app.source_pane);
    if let Some(mut active) = pane.active.take() {
        let message = app.session.mailbox.messages.get(&active.id).cloned();
        if active.opening != app.session.openings() || app.session.sel.messages.as_slice() != [active.id] || message.is_none() {
            pane.retire(active);
        } else {
            if let Some(message) = message {
                // Flags, read state or sync bookkeeping replace the message but not its
                // source: the reader keeps its token and page.
                if !Arc::ptr_eq(&active.message, &message) && !same_source(&active.message, &message) && !active.body.downloading {
                    if let Some(token) = active.body.token {
                        pane.retiring.push((active.id, active.body.command, token));
                    }
                    active.body = Reader::new("mail.sourcePage");
                }
                active.message = message;
            }
            active.body.step(app, active.id);
            if let Some(export) = &mut active.export {
                export.step(app, active.id);
            }
            pane.active = Some(active);
        }
    }
    pane.retiring.retain(
        |(id, command, token)| matches!(app.session.run(command,&json!({"id":id,"token":token,"close":true})),Ok(value) if value["state"]=="closing"),
    );
    let retiring = &mut pane.retiring;
    let prepared = pane.active.as_mut().and_then(|active| {
        let export = active.export.as_ref()?;
        if let Some(error) = &export.error {
            app.status(error.clone());
            if let Some(token) = export.token {
                retiring.push((active.id, export.command, token));
            }
            active.export = None;
            return None;
        }
        if export.wanted {
            return None;
        }
        let token = export.token?;
        active.export = None;
        Some((active.id, token))
    });
    app.source_pane = pane;
    if let Some((id, token)) = prepared {
        // The picker owns this token from here; cancellation also closes it.
        if let Err(why) = app.run("file.exportMessage", json!({"id":id,"token":token})) {
            let _ = app.session.run("mail.exportEml", &json!({"id":id,"token":token,"close":true}));
            app.status(why);
        }
    }
}

/// The source of the message being read, in place of its body: a line that says what it is,
/// with Copy and Export .eml, and the source itself, coloured, in one scroll.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let mut action = None;
    let mut copy = false;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 8, bottom: 12 }).show(ui, |ui| {
        let Some(active) = &mut app.source_pane.active else { return };
        let id = active.id;
        let exporting = active.export.is_some();
        let reader = &mut active.body;
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        let caption = ui.horizontal_wrapped(|ui| {
            let html = reader.answer.as_ref().is_some_and(|answer| answer["format"] == "html");
            let title = if html {
                tl!("HTML source")
            } else if reader.answer.is_some() {
                tl!("Plain-text source")
            } else {
                tl!("Message source")
            };
            ui.label(egui::RichText::new(title).font(crate::theme::semibold(12.5)).color(t.text));
            if let Some(bytes) = reader.answer.as_ref().and_then(|answer| answer["bytes"].as_u64()) {
                ui.label(egui::RichText::new(mailcraft_model::size_label_in(crate::i18n::current(), bytes.max(1))).size(11.5).color(t.text_dim));
            }
            let copied = ui.add_enabled(!reader.pages.is_empty(), egui::Button::new(tl!("Copy")).small()).on_hover_text(tl!("Copy the source shown here"));
            copy = copied.clicked();
            let export = ui
                .add_enabled(!exporting && app.file_dialog.is_none(), egui::Button::new(tl!("Export .eml")).small())
                .on_hover_text(tl!("Save the complete message as an .eml file"));
            app.hit.add("sourceExport", id, export.rect);
            if export.clicked() {
                action = Some(json!({"export":true}));
            }
            if exporting {
                ui.spinner();
                ui.label(egui::RichText::new(tl!("Preparing complete message…")).size(11.5).color(t.text_dim));
            }
        });
        app.hit.add("sourceLabel", id, caption.response.rect);
        let retained_original = app.session.mailbox.message(id).is_some_and(|message| !message.draft && message.original.is_some());
        let (description, details) = if retained_original {
            (
                tl!("Text only: nothing runs or downloads. Export preserves the retained protected MIME body."),
                tl!(
                    "The signed or encrypted MIME body is preserved byte for byte; safe message headers are reconstructed. Export fails if the retained original cannot be read completely."
                ),
            )
        } else {
            (
                tl!("Text only: nothing runs or downloads. Export reconstructs MIME; original bytes are unavailable."),
                tl!(
                    "The retained headers, decoded bodies and attachments are exported completely. Original MIME nesting, encoding and signatures cannot be recreated."
                ),
            )
        };
        ui.add(egui::Label::new(egui::RichText::new(description).color(t.text_dim).size(11.5)).wrap()).on_hover_text(details);
        if let Some(error) = &reader.error {
            ui.colored_label(t.text, error);
            return;
        }
        if reader.pages.is_empty() {
            if reader.wanted {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(if reader.downloading { tl!("Downloading the complete message…") } else { tl!("Preparing source…") });
                });
            }
            return;
        }
        let room = ui.available_size().max(vec2(1.0, 1.0));
        let mut more = false;
        egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
            let scrolled = egui::ScrollArea::both()
                .id_salt(("source-body", id))
                .max_height((room.y - 12.0).max(1.0))
                .auto_shrink([false, false])
                .show_viewport(ui, |ui, viewport| {
                    let (body, wants_more) = pages(ui, reader, viewport, &t);
                    more = wants_more;
                    body
                });
            app.hit.add("sourceBody", id, scrolled.inner);
            app.hit.add("sourceViewport", id, scrolled.inner_rect);
        });
        // The next page, once the end of what is here comes near: one at a time.
        if more && !reader.wanted && reader.error.is_none() && reader.next().is_some() {
            action = Some(json!({"page":"next"}));
        }
    });
    if copy && let Some(active) = &app.source_pane.active {
        let mut text: String = active.body.pages.iter().map(|page| &*page.text).collect();
        text.push_str(&active.body.carry);
        ui.ctx().copy_text(text);
    }
    if let Some(params) = action
        && let Err(why) = command(app, &params)
    {
        app.status(why);
    }
}

/// The pages of `reader` one under the other, laying out only those near `viewport` (in the
/// scroll's own coordinates). Where they went, and whether the end is near enough to want more.
fn pages(ui: &mut Ui, reader: &mut Reader, viewport: Rect, t: &Tokens) -> (Rect, bool) {
    let font = crate::theme::mono(FONT_SIZE);
    let (row, glyph) = ui.fonts_mut(|f| (f.row_height(&font), f.glyph_width(&font, 'M')));
    let key = LaidKey { palette: [t.text, t.text_dim, t.blue, t.green], scale: ui.ctx().pixels_per_point() };
    let origin = ui.min_rect().min;
    let near = viewport.expand2(vec2(0.0, viewport.height()));
    let mut y = 0.0;
    let mut width: f32 = viewport.width();
    for page in &mut reader.pages {
        // Known once laid out; until then, what fixed-width text of its lines and columns takes.
        let guess = vec2(page.columns as f32 * glyph, page.lines as f32 * row);
        let size = page.laid.as_ref().map_or(guess, |(_, galley)| galley.size());
        let span = Rect::from_min_size(pos2(0.0, y), size);
        if span.intersects(near) {
            if page.laid.as_ref().is_none_or(|(laid, _)| *laid != key) {
                let job = job(&page.text, &page.runs, &font, t);
                page.laid = Some((key, ui.fonts_mut(|f| f.layout_job(job))));
            }
            if let Some((_, galley)) = &page.laid {
                let rect = Rect::from_min_size(origin + vec2(0.0, y), galley.size());
                ui.put(rect, egui::Label::new(galley.clone()).selectable(true));
            }
        } else {
            // Out of sight: its layout goes; its size is known again by its lines.
            page.laid = None;
        }
        let size = page.laid.as_ref().map_or(guess, |(_, galley)| galley.size());
        width = width.max(size.x);
        y += size.y;
    }
    let note = if reader.capped() {
        Some(crate::i18n::fmt(tl!("Source truncated at {mb} MB. Export .eml has all of it."), &[("mb", &(MAX_LOADED >> 20).to_string())]))
    } else if reader.wanted {
        Some(tl!("Loading more…").to_string())
    } else {
        None
    };
    if let Some(note) = note {
        let rect = Rect::from_min_size(origin + vec2(0.0, y + 6.0), vec2(width, row + 4.0));
        ui.put(rect, egui::Label::new(egui::RichText::new(note).size(11.5).italics().color(t.text_dim)));
        y += row + 10.0;
    }
    ui.set_min_size(vec2(width, y));
    (Rect::from_min_size(origin, vec2(width, y)), viewport.max.y + viewport.height() * AHEAD >= y)
}

/// What a run of source text is, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    HeaderName,
    HeaderValue,
    Boundary,
    /// `<`, `>`, a tag's name.
    Tag,
    Attribute,
    String,
    Comment,
    /// A base64 or quoted-printable line.
    Encoded,
}

/// Where the colouring stands between two lines (and two pages).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scan {
    /// Among header lines: at the start, and after a MIME boundary until the blank line.
    pub headers: bool,
    /// Past the first line (the start of the source is a header block only if it looks like one).
    pub begun: bool,
    pub in_tag: bool,
    /// The quote a string in a tag is in.
    pub quote: Option<u8>,
    pub in_comment: bool,
}

/// Add a run that ends at `end`, joined to the one before when it is of the same kind.
fn push(runs: &mut Vec<(usize, Kind)>, end: usize, kind: Kind) {
    match runs.last_mut() {
        Some(last) if last.1 == kind => last.0 = last.0.max(end),
        Some(last) if end <= last.0 => {}
        _ if end == 0 => {}
        _ => runs.push((end, kind)),
    }
}

/// The runs of `text` and what each is, as `(end, kind)` in order, going on from `scan` and
/// leaving it where the text ends. Line by line: header lines, boundaries and encoded lines are
/// told by their shape; the rest is read as HTML (text outside tags stays text).
pub(crate) fn spans(text: &str, scan: &mut Scan) -> Vec<(usize, Kind)> {
    let mut runs: Vec<(usize, Kind)> = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let bare = line.trim_end_matches(['\r', '\n']);
        if !scan.begun {
            scan.begun = true;
            scan.headers = header_name(bare).is_some();
        }
        let plain = !scan.in_tag && !scan.in_comment && scan.quote.is_none();
        if plain && is_boundary(bare) {
            push(&mut runs, at, Kind::Boundary);
            scan.headers = true;
            continue;
        }
        if scan.headers && plain {
            if bare.is_empty() {
                scan.headers = false;
                push(&mut runs, at, Kind::Text);
                continue;
            }
            if bare.starts_with([' ', '\t']) {
                push(&mut runs, at, Kind::HeaderValue);
                continue;
            }
            if let Some(name) = header_name(bare) {
                push(&mut runs, start + name + 1, Kind::HeaderName);
                push(&mut runs, at, Kind::HeaderValue);
                continue;
            }
            scan.headers = false;
        }
        if plain && is_encoded(bare) {
            push(&mut runs, at, Kind::Encoded);
            continue;
        }
        html_line(line, start, scan, &mut runs);
    }
    runs
}

/// One line read as HTML, from where `scan` stands.
fn html_line(line: &str, start: usize, scan: &mut Scan, runs: &mut Vec<(usize, Kind)>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    // Inside a tag, a name is the tag's own until the first space; then attributes.
    let mut named = !scan.in_tag;
    while i < bytes.len() {
        let b = bytes[i];
        if scan.in_comment {
            match line[i..].find("-->") {
                Some(p) => {
                    i += p + 3;
                    scan.in_comment = false;
                }
                None => i = line.len(),
            }
            push(runs, start + i, Kind::Comment);
            continue;
        }
        if let Some(q) = scan.quote {
            match bytes[i..].iter().position(|c| *c == q) {
                Some(p) => {
                    i += p + 1;
                    scan.quote = None;
                }
                None => i = line.len(),
            }
            push(runs, start + i, Kind::String);
            continue;
        }
        if scan.in_tag {
            let kind = match b {
                b'>' => {
                    scan.in_tag = false;
                    Kind::Tag
                }
                b'"' | b'\'' => {
                    scan.quote = Some(b);
                    Kind::String
                }
                b' ' | b'\t' | b'\r' | b'\n' => {
                    named = false;
                    Kind::Text
                }
                b'=' | b'/' => Kind::Tag,
                _ if named => Kind::Tag,
                _ => Kind::Attribute,
            };
            // (Any byte that is not ASCII belongs to a name: it goes with the bytes after it.)
            i += 1;
            while i < bytes.len() && !line.is_char_boundary(i) {
                i += 1;
            }
            push(runs, start + i, kind);
            continue;
        }
        if line[i..].starts_with("<!--") {
            scan.in_comment = true;
            i += 4;
            push(runs, start + i, Kind::Comment);
            continue;
        }
        if b == b'<' && bytes.get(i + 1).is_some_and(|n| n.is_ascii_alphabetic() || matches!(n, b'/' | b'!' | b'?')) {
            scan.in_tag = true;
            named = true;
            i += 1;
            push(runs, start + i, Kind::Tag);
            continue;
        }
        // Text, up to the next `<`.
        let step = line[i..].chars().next().map_or(1, char::len_utf8);
        i = line[i + step..].find('<').map_or(line.len(), |p| i + step + p);
        push(runs, start + i, Kind::Text);
    }
}

/// `Name:` at the start of a header line: where its colon is.
fn header_name(line: &str) -> Option<usize> {
    let colon = line.find(':')?;
    (colon > 0 && colon <= 76 && line[..colon].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')).then_some(colon)
}

/// A MIME boundary line: `--` and a boundary without spaces, with or without the closing `--`.
fn is_boundary(line: &str) -> bool {
    line.len() > 6 && line.len() <= 76 && line.starts_with("--") && !line.contains([' ', '\t', '<', '>']) && !line[2..].starts_with('-')
}

/// A line of base64, or of quoted-printable that carries encoded bytes or ends in a soft break.
fn is_encoded(line: &str) -> bool {
    if line.len() >= 60 && line.len() <= 1000 && line.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')) {
        // Base64: upper and lower case and digits mixed, as no word is.
        return line.bytes().any(|b| b.is_ascii_digit())
            && line.bytes().any(|b| b.is_ascii_uppercase())
            && line.bytes().any(|b| b.is_ascii_lowercase());
    }
    let soft = line.len() >= 60 && line.ends_with('=') && !line.ends_with("==");
    let escapes = line
        .as_bytes()
        .windows(3)
        .filter(|w| w[0] == b'=' && w[1].is_ascii_hexdigit() && w[2].is_ascii_hexdigit() && !w[1].is_ascii_lowercase() && !w[2].is_ascii_lowercase())
        .count();
    soft || escapes >= 3
}

/// The colour of a kind of run, from the window's tokens.
fn color(kind: Kind, t: &Tokens) -> Color32 {
    match kind {
        Kind::Text | Kind::HeaderValue => t.text,
        Kind::HeaderName => t.accent_text,
        Kind::Boundary => t.icon_purple,
        Kind::Tag => t.blue,
        Kind::Attribute => t.orange,
        Kind::String => t.green,
        Kind::Comment | Kind::Encoded => t.text_dim,
    }
}

/// A page's text as a layout job: one section per run, never wrapped.
fn job(text: &str, runs: &[(usize, Kind)], font: &egui::FontId, t: &Tokens) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = f32::INFINITY;
    let format = |kind: Kind| TextFormat { font_id: font.clone(), color: color(kind, t), italics: kind == Kind::Comment, ..Default::default() };
    let mut from = 0;
    for &(end, kind) in runs {
        let end = end.min(text.len());
        if end <= from || !text.is_char_boundary(end) {
            continue;
        }
        job.append(&text[from..end], 0.0, format(kind));
        from = end;
    }
    if from < text.len() {
        job.append(&text[from..], 0.0, format(Kind::Text));
    }
    job
}
