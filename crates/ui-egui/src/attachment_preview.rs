// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! On-demand preview of an attachment: of a message read, or of a message being written
//! (v0.12.14). It is shown in the Preview Pane beside the message (`view.previewPane`), or, with
//! that pane off, in place of the message's body.
//!
//! What is shown and by what:
//! - text, CSV and TSV, HTML, a message (`.eml`) and a ZIP file's list of files are read here, in
//!   this process, within bounds: nothing in them runs and nothing comes from the web;
//! - pictures (PNG, JPEG, GIF, WebP, icons) are decoded here too, on the app's worker when it has
//!   one ([`Render::Picture`]);
//! - PDF, Office, OpenDocument, Photoshop, SVG, Illustrator and Fig files are drawn a page at a
//!   time by the Craft content runtime that comes with MailCraft ([`Render::Craft`]), and, when the
//!   owner turned it on in File › Options, by an installed PdfCraft or PhotoCraft ([`Render::Helper`]).
//!
//! A file Microsoft Defender flagged is never previewed, nor a file forwarded from a message in
//! Junk E-mail or with a flagged file. In Junk E-mail, only the attachment's menu starts a preview.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc::Receiver};
use std::time::Duration;

use mailcraft_engine::{Module, Pane};
use mailcraft_model::{Attachment, Body, Id, Message};
use serde_json::{Value, json};

use crate::MailApp;
use crate::reading::Place;
use crate::theme::Tokens;

/// The largest attachment previewed (the Craft runtime's input allowance).
pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PAGES: u32 = 10_000;
pub const MAX_SIDE: u32 = 3_072;
/// Most of a text file shown.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// The largest HTML file and message (`.eml`) shown.
const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Most rows and columns of a CSV file shown, and characters of a cell.
pub const MAX_ROWS: usize = 200;
const MAX_COLUMNS: usize = 30;
const MAX_CELL: usize = 200;
/// Most files of a ZIP file listed.
pub const MAX_ENTRIES: usize = 1_000;
/// A picture is shown at most this many pixels on a side.
const MAX_PICTURE_SIDE: u32 = 2_048;
const RETRY_DELAY: Duration = Duration::from_millis(25);
const MAX_RETRIES: u8 = 120;

/// The types of file the Craft content runtime draws pages of (`attachment_pages::EXTENSIONS` in
/// the engine, which isn't built for the web), and TIFF, which only PhotoCraft draws.
const PAGE_TYPES: &[&str] =
    &["pdf", "docx", "docm", "doc", "odt", "xlsx", "xlsm", "xls", "xlsb", "ods", "pptx", "psd", "psb", "svg", "svgz", "ai", "fig", "tif"];

/// Text files shown as they are, in a fixed-width font.
const CODE_TYPES: &[&str] = &[
    "log",
    "json",
    "xml",
    "yaml",
    "yml",
    "ics",
    "vcf",
    "ini",
    "cfg",
    "conf",
    "toml",
    "css",
    "js",
    "ts",
    "rs",
    "py",
    "c",
    "h",
    "cpp",
    "cs",
    "java",
    "go",
    "sh",
    "ps1",
    "bat",
    "sql",
    "diff",
    "patch",
    "srt",
    "properties",
];

/// A file type a PdfCraft or PhotoCraft helper reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Pdf,
    Png,
    Jpeg,
    Tiff,
    Webp,
}

impl Format {
    /// The fixed extension used for the temporary input name.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Tiff => "tif",
            Self::Webp => "webp",
        }
    }

    /// Check the file signature before handing bytes to the helper.
    pub fn matches(self, data: &[u8]) -> bool {
        match self {
            Self::Pdf => data.starts_with(b"%PDF-"),
            Self::Png => data.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Jpeg => data.starts_with(b"\xff\xd8\xff"),
            Self::Tiff => data.starts_with(b"II*\0") || data.starts_with(b"MM\0*"),
            Self::Webp => data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP"),
        }
    }
}

/// What kind of preview a file gets, by its name's extension first and then its MIME type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Text; `mono`: in a fixed-width font (code, logs, data).
    Text {
        mono: bool,
    },
    /// Comma- or tab-separated values, by their separator.
    Table(u8),
    Html,
    /// A message (`.eml`).
    Message,
    /// A ZIP file's list of files.
    Archive,
    Picture,
    /// Pages drawn by the Craft runtime or a helper, by the type's extension.
    Pages(&'static str),
    Unsupported,
}

/// The kind of preview for a file named `name` of MIME type `mime`.
pub fn kind_of(name: &str, mime: &str) -> Kind {
    let extension = name.rsplit_once('.').map(|(_, e)| e.trim().to_ascii_lowercase()).unwrap_or_default();
    let by_extension = match extension.as_str() {
        "txt" | "text" | "md" | "markdown" => Some(Kind::Text { mono: false }),
        e if CODE_TYPES.contains(&e) => Some(Kind::Text { mono: true }),
        "csv" => Some(Kind::Table(b',')),
        "tsv" | "tab" => Some(Kind::Table(b'\t')),
        "html" | "htm" => Some(Kind::Html),
        "eml" => Some(Kind::Message),
        "zip" => Some(Kind::Archive),
        "png" | "jpg" | "jpeg" | "jfif" | "gif" | "webp" | "ico" => Some(Kind::Picture),
        "tiff" => Some(Kind::Pages("tif")),
        e => PAGE_TYPES.iter().copied().find(|t| *t == e).map(Kind::Pages),
    };
    if let Some(kind) = by_extension {
        return kind;
    }
    let mime = mime.trim().to_ascii_lowercase();
    match mime.as_str() {
        "text/csv" => Kind::Table(b','),
        "text/tab-separated-values" => Kind::Table(b'\t'),
        "text/html" => Kind::Html,
        "message/rfc822" => Kind::Message,
        "application/zip" | "application/x-zip-compressed" => Kind::Archive,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/x-icon" | "image/vnd.microsoft.icon" => Kind::Picture,
        "application/pdf" => Kind::Pages("pdf"),
        "image/tiff" => Kind::Pages("tif"),
        "image/svg+xml" => Kind::Pages("svg"),
        m if m.starts_with("text/") => Kind::Text { mono: m != "text/plain" },
        _ => Kind::Unsupported,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unavailable,
    Invalid,
    TooLarge,
    TimedOut,
    Busy,
    Failed,
    /// No preview is made for this type of file.
    Unsupported,
    /// Microsoft Defender flagged it, or it was forwarded from a message that isn't trusted.
    Refused,
}

impl Error {
    /// Explain a refused or failed preview in the interface's language.
    fn text(self) -> &'static str {
        match self {
            Self::Unavailable => tl!("No preview helper is available for this attachment."),
            Self::Invalid => tl!("This attachment could not be previewed."),
            Self::TooLarge => tl!("This attachment is too large to preview."),
            Self::TimedOut => tl!("The attachment preview timed out."),
            Self::Busy => tl!("Wait for the other attachment preview to finish."),
            Self::Failed => tl!("The attachment preview could not be opened."),
            Self::Unsupported => tl!("No preview for this file type."),
            Self::Refused => tl!("This attachment isn't previewed, to keep you safe."),
        }
    }
}

/// How the app's worker makes a preview of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Render {
    /// A picture, decoded with [`decode_picture`].
    Picture,
    /// A page drawn by the Craft content runtime, for a file of this type (extension).
    Craft(&'static str),
    /// A page drawn by an installed PdfCraft or PhotoCraft.
    Helper(Format),
}

type ReadAttachment = Arc<dyn Fn() -> Result<Arc<Vec<u8>>, Error> + Send + Sync>;

pub struct Request {
    read: ReadAttachment,
    pub render: Render,
    pub page: u32,
    pub pages: Option<u32>,
    pub cancel: Arc<AtomicBool>,
}

impl Request {
    /// Load and validate the attachment on the renderer's worker thread.
    pub fn load(&self) -> Result<Arc<Vec<u8>>, Error> {
        let bytes = (self.read)()?;
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(Error::TooLarge);
        }
        if !fits(self.render, &bytes) {
            return Err(Error::Invalid);
        }
        Ok(bytes)
    }
}

/// Whether `data` is what `render` is to be given: a helper's type by its signature, a PDF for
/// the Craft runtime's PDF core.
fn fits(render: Render, data: &[u8]) -> bool {
    match render {
        Render::Helper(format) => format.matches(data),
        Render::Craft("pdf") => data.starts_with(b"%PDF-"),
        Render::Craft(_) | Render::Picture => true,
    }
}

pub struct Page {
    pub image: egui::ColorImage,
    pub pages: u32,
}

/// The app's preview workers: the installed helpers it found, the types the Craft content runtime
/// it comes with draws, and how a page is asked for.
pub struct Host {
    pub pdf: bool,
    pub images: bool,
    /// The types (extensions) the Craft content runtime draws; empty without the runtime.
    pub craft: Vec<String>,
    pub render: Box<dyn Fn(Request) -> Receiver<Result<Page, Error>>>,
}

/// A picture's pixels, decoded within bounds and made at most [`MAX_PICTURE_SIDE`] on a side: only
/// PNG, JPEG, GIF, WebP and icons, by their contents, whatever the file is named.
pub fn decode_picture(bytes: &[u8]) -> Result<egui::ColorImage, Error> {
    use image::ImageFormat;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|_| Error::Invalid)?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP | ImageFormat::Ico)) {
        return Err(Error::Invalid);
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut picture = reader.decode().map_err(|_| Error::Invalid)?;
    if picture.width() > MAX_PICTURE_SIDE || picture.height() > MAX_PICTURE_SIDE {
        picture = picture.thumbnail(MAX_PICTURE_SIDE, MAX_PICTURE_SIDE);
    }
    let rgba = picture.into_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

/// Where a previewed file is: a message's, or one of a message being written (by its id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    Message(Id),
    Draft(u64),
}

impl Origin {
    /// Its number in the hit map: the message's, or the message being written's.
    fn key(self) -> u64 {
        match self {
            Self::Message(id) | Self::Draft(id) => id,
        }
    }
}

/// What a preview read in this process shows.
#[derive(Clone)]
enum Content {
    /// Text; `cut`: only its start is shown.
    Text {
        text: Arc<str>,
        cut: bool,
        mono: bool,
    },
    Table {
        rows: Arc<Vec<Vec<String>>>,
        cut: bool,
    },
    Message(Arc<Message>),
    Html(Arc<Message>),
    /// A ZIP file's files, by name and size, and how many it has.
    Entries {
        entries: Arc<Vec<(String, u64)>>,
        total: usize,
    },
}

pub(crate) struct Preview {
    origin: Origin,
    index: usize,
    name: String,
    size: String,
    viewport: egui::ViewportId,
    /// The window of its own it is shown in (the hit map's number for it; 0: the main window):
    /// windows drawn inside the main one share its viewport.
    spot: u64,
    mailbox: u64,
    attachment: Attachment,
    read: ReadAttachment,
    render: Option<Render>,
    page: u32,
    pages: Option<u32>,
    pending: Option<Receiver<Result<Page, Error>>>,
    retries: u8,
    retry_at: Option<f64>,
    cancel: Arc<AtomicBool>,
    texture: Option<egui::TextureHandle>,
    content: Option<Content>,
    error: Option<Error>,
    zoom: f32,
}

pub(crate) struct Click {
    origin: Origin,
    index: usize,
    viewport: egui::ViewportId,
    spot: u64,
    mailbox: u64,
    attachment: Attachment,
    at: f64,
}

impl Drop for Preview {
    /// Stop a renderer when its preview is replaced or closed.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Whether attachments are previewed at all: the Preview Pane is on, or the owner turned on the
/// installed helpers in File › Options.
fn enabled(app: &MailApp) -> bool {
    app.ui.attachment_previews || app.session.view.preview_pane
}

/// How a preview of a file of `kind` is made here, if it can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// Read in this process now.
    Local,
    /// Made by the app's worker.
    Host(Render),
}

fn route(app: &MailApp, kind: Kind) -> Option<Route> {
    let host = app.services.attachment_preview.as_ref();
    match kind {
        Kind::Text { .. } | Kind::Table(_) | Kind::Html | Kind::Message | Kind::Archive => Some(Route::Local),
        Kind::Picture => Some(if host.is_some() { Route::Host(Render::Picture) } else { Route::Local }),
        Kind::Pages(extension) => {
            let host = host?;
            if host.craft.iter().any(|t| t == extension) {
                return Some(Route::Host(Render::Craft(extension)));
            }
            let format = match extension {
                "pdf" if host.pdf => Format::Pdf,
                "tif" if host.images => Format::Tiff,
                _ => return None,
            };
            app.ui.attachment_previews.then_some(Route::Host(Render::Helper(format)))
        }
        Kind::Unsupported => None,
    }
}

/// Whether this attachment can be offered in the explicit preview menu. One Microsoft Defender
/// flagged is never previewed: it is opened, if at all, through the owner's own "Open anyway".
pub(crate) fn available(app: &MailApp, id: Id, index: usize) -> bool {
    let origin = Origin::Message(id);
    enabled(app)
        && !refused(app, origin, index)
        && attachment(app, origin, index)
            .filter(|part| (1..=MAX_INPUT_BYTES).contains(&part.size()))
            .is_some_and(|part| route(app, kind_of(&part.name, &part.mime)).is_some())
}

/// Show the opt-in control for the helpers this installation found.
pub(crate) fn options(app: &mut MailApp, ui: &mut egui::Ui) {
    let Some(host) = &app.services.attachment_preview else { return };
    let label = match (host.pdf, host.images) {
        (true, true) => tl!("Preview attachments with local PdfCraft and PhotoCraft"),
        (true, false) => tl!("Preview attachments with local PdfCraft"),
        (false, true) => tl!("Preview attachments with local PhotoCraft"),
        (false, false) => return,
    };
    let response = ui
        .checkbox(&mut app.ui.attachment_previews, label)
        .on_hover_text(tl!("The local program receives the selected attachment and runs with your permissions."));
    app.hit.add("attachmentPreviewOption", 0, response.rect);
    if response.changed() && !app.ui.attachment_previews {
        app.attachment_preview = None;
        app.pending_attachment_preview = None;
    }
}

/// Whether a chip click can preview the attachment without opening its menu. With the Preview
/// Pane on, any file can be clicked: the pane says when there is no preview for it.
pub(crate) fn single_click_allowed(app: &MailApp, origin: Origin, index: usize) -> bool {
    let in_junk = matches!(origin, Origin::Message(id) if in_junk(app, id));
    enabled(app)
        && !in_junk
        && !refused(app, origin, index)
        && attachment(app, origin, index).is_some_and(|part| {
            app.session.view.preview_pane || (part.size() <= MAX_INPUT_BYTES && route(app, kind_of(&part.name, &part.mime)).is_some())
        })
}

/// Apply the mailbox's Junk policy to the message, including descendants of Junk.
fn in_junk(app: &MailApp, id: Id) -> bool {
    app.session.mailbox.message(id).is_some_and(|m| app.session.mailbox.in_junk(m.folder))
}

/// Whether the file is never previewed: Microsoft Defender flagged it; or, being sent, it was
/// forwarded from a message in Junk E-mail or one with a flagged file.
fn refused(app: &MailApp, origin: Origin, index: usize) -> bool {
    match origin {
        Origin::Message(id) => crate::attachment_scan::flagged(app, id, index).is_some(),
        Origin::Draft(id) => {
            let Some(c) = app.session.composing_by_id(id) else { return true };
            let Some(part) = c.attachments.get(index) else { return true };
            crate::attachment_scan::flagged_file(app, part)
                || c.source.iter().chain(&c.also).filter_map(|id| app.session.mailbox.message(*id)).any(|m| {
                    let untrusted = app.session.mailbox.in_junk(m.folder)
                        || (0..m.attachments.len()).any(|i| crate::attachment_scan::flagged(app, m.id, i).is_some());
                    untrusted
                        && (part.mime.eq_ignore_ascii_case("message/rfc822")
                            || m.attachments.iter().any(|f| f.name == part.name && f.size() == part.size()))
                })
        }
    }
}

/// Defer the chip action until a double-click can be ruled out.
pub(crate) fn queue_click(app: &mut MailApp, ctx: &egui::Context, origin: Origin, index: usize) {
    let Some(attachment) = attachment(app, origin, index).cloned() else { return };
    let delay = ctx.options(|o| o.input_options.max_double_click_delay).clamp(0.0, 2.0);
    let delay = if delay.is_finite() { delay } else { 0.3 };
    app.pending_attachment_preview = Some(Click {
        origin,
        index,
        viewport: ctx.viewport_id(),
        spot: app.hit.window(),
        mailbox: app.session.openings(),
        attachment,
        at: ctx.input(|i| i.time) + delay,
    });
    crate::software::repaint_after(ctx, Duration::from_secs_f64(delay + 0.001));
}

/// Discard the queued single-click action without starting a helper.
pub(crate) fn cancel_click(app: &mut MailApp) {
    app.pending_attachment_preview = None;
}

/// Start the queued action after checking that it still names the same attachment.
pub(crate) fn finish_click(app: &mut MailApp, ctx: &egui::Context, origin: Origin) {
    let spot = app.hit.window();
    let Some(click) =
        app.pending_attachment_preview.as_ref().filter(|click| click.viewport == ctx.viewport_id() && click.spot == spot && click.origin == origin)
    else {
        return;
    };
    let remaining = click.at - ctx.input(|i| i.time);
    if remaining >= 0.0 {
        crate::software::repaint_after(ctx, Duration::from_secs_f64(remaining + 0.001));
        return;
    }
    let Some(click) = app.pending_attachment_preview.take() else { return };
    if single_click_allowed(app, click.origin, click.index)
        && attachment(app, click.origin, click.index) == Some(&click.attachment)
        && let Err(error) = open_in(app, click.viewport, click.origin, click.index)
    {
        app.status(error);
    }
}

/// Look up an attachment without loading its contents.
fn attachment(app: &MailApp, origin: Origin, index: usize) -> Option<&Attachment> {
    match origin {
        Origin::Message(id) => app.session.mailbox.message(id)?.attachments.get(index),
        Origin::Draft(id) => app.session.composing_by_id(id)?.attachments.get(index),
    }
}

/// Whether this chip owns the preview currently shown where it is drawn.
pub(crate) fn selected(app: &MailApp, viewport: egui::ViewportId, origin: Origin, index: usize) -> bool {
    let spot = app.hit.window();
    app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && p.spot == spot && p.origin == origin && p.index == index)
}

/// Preview attachment `index` of message `id` where it is being drawn.
pub(crate) fn open(app: &mut MailApp, viewport: egui::ViewportId, id: Id, index: usize) -> Result<(), String> {
    open_in(app, viewport, Origin::Message(id), index)
}

/// Replace any old preview and start the selected attachment's: read here, or its first page or
/// picture asked of the app's worker.
pub(crate) fn open_in(app: &mut MailApp, viewport: egui::ViewportId, origin: Origin, index: usize) -> Result<(), String> {
    cancel_click(app);
    if !enabled(app) {
        return Err(tl!("Turn on the Preview Pane in the View tab.").into());
    }
    let spot = app.hit.window();
    if refused(app, origin, index) {
        return Err(Error::Refused.text().into());
    }
    let part = attachment(app, origin, index).ok_or_else(|| Error::Invalid.text())?;
    if part.size() > MAX_INPUT_BYTES {
        return Err(Error::TooLarge.text().into());
    }
    let kind = kind_of(&part.name, &part.mime);
    let route = route(app, kind);
    // Without the pane, a file with no preview stays a message; the pane says so in its place.
    let missing = if kind == Kind::Unsupported { Error::Unsupported } else { Error::Unavailable };
    if route.is_none() && !app.session.view.preview_pane {
        return Err(missing.text().into());
    }
    let same = app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && p.spot == spot && p.origin == origin && p.index == index);
    if same && app.attachment_preview.as_ref().is_some_and(|preview| preview.error.is_none()) {
        return Ok(());
    }
    if let Some(Route::Host(render)) = route
        && !part.data.is_empty()
        && !fits(render, &part.data)
    {
        return Err(Error::Invalid.text().into());
    }
    let name = mailcraft_engine::opening::file_name(&part.name);
    let size = mailcraft_model::size_label_in(crate::i18n::current(), part.size() as u64);
    let read = app.session.attachment_reader(part).map_err(|_| Error::Invalid.text())?;
    let attachment = part.clone();
    // Dropping a preview tells the worker to stop before the next render starts.
    app.attachment_preview = None;
    let mut preview = Preview {
        origin,
        index,
        name,
        size,
        viewport,
        spot,
        mailbox: app.session.openings(),
        attachment,
        read: Arc::new(move || read().map_err(|_| Error::Invalid)),
        render: None,
        page: 1,
        pages: None,
        pending: None,
        retries: 0,
        retry_at: None,
        cancel: Arc::new(AtomicBool::new(false)),
        texture: None,
        content: None,
        error: None,
        zoom: 1.0,
    };
    match route {
        Some(Route::Host(render)) => {
            preview.render = Some(render);
            if let Some(host) = &app.services.attachment_preview {
                preview.start(host, 1);
            }
        }
        Some(Route::Local) => preview.read_here(kind),
        None => preview.error = Some(missing),
    }
    app.attachment_preview = Some(preview);
    Ok(())
}

/// Handle an owner's script request, which still requires an explicit menu in Junk.
pub(crate) fn command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let id = params.get("id").and_then(Value::as_u64).or_else(|| app.session.sel.focus()).ok_or_else(|| Error::Invalid.text())?;
    let index = params.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| Error::Invalid.text())?;
    if app.session.sel.focus() != Some(id) || app.session.sel.messages.len() != 1 {
        return Err(tl!("Select the message before previewing its attachment.").into());
    }
    if in_junk(app, id) {
        return Err(tl!("In Junk E-mail, preview an attachment from its menu.").into());
    }
    open(app, egui::ViewportId::ROOT, id, index)?;
    Ok(json!({"preview": true}))
}

impl Preview {
    /// Start a page with its own retry budget.
    fn start(&mut self, host: &Host, page: u32) {
        self.page = page;
        self.texture = None;
        self.error = None;
        self.retries = 0;
        self.retry_at = None;
        self.submit(host);
    }

    /// Submit the current page to the app's worker.
    fn submit(&mut self, host: &Host) {
        let Some(render) = self.render else { return };
        self.pending =
            Some((host.render)(Request { read: self.read.clone(), render, page: self.page, pages: self.pages, cancel: self.cancel.clone() }));
    }

    /// Read a file shown by this process: its text, rows, message or list of files, or (with no
    /// worker to decode it on) its picture.
    fn read_here(&mut self, kind: Kind) {
        let bytes = match (self.read)() {
            Ok(bytes) if bytes.len() > MAX_INPUT_BYTES => return self.error = Some(Error::TooLarge),
            Ok(bytes) => bytes,
            Err(error) => return self.error = Some(error),
        };
        let content = match kind {
            Kind::Text { mono } => {
                let (text, cut) = text_of(&bytes, MAX_TEXT_BYTES);
                Content::Text { text: text.into(), cut, mono }
            }
            Kind::Table(separator) => {
                let (text, cut) = text_of(&bytes, MAX_TEXT_BYTES);
                let (rows, more) = table_of(&text, char::from(separator));
                Content::Table { rows: Arc::new(rows), cut: cut || more }
            }
            Kind::Html if bytes.len() > MAX_HTML_BYTES => return self.error = Some(Error::TooLarge),
            Kind::Html => {
                let (html, _) = text_of(&bytes, MAX_HTML_BYTES);
                Content::Html(Arc::new(Message { body: Body { text: String::new(), html: Some(html) }, ..Message::default() }))
            }
            Kind::Message if bytes.len() > MAX_MESSAGE_BYTES => return self.error = Some(Error::TooLarge),
            Kind::Message => Content::Message(Arc::new(mailcraft_engine::mime::parse(&bytes))),
            Kind::Archive => match entries_of(&bytes) {
                Some((entries, total)) => Content::Entries { entries: Arc::new(entries), total },
                None => return self.error = Some(Error::Invalid),
            },
            Kind::Picture => {
                let (send, receive) = std::sync::mpsc::channel();
                let _ = send.send(decode_picture(&bytes).map(|image| Page { image, pages: 1 }));
                self.pending = Some(receive);
                return;
            }
            Kind::Pages(_) | Kind::Unsupported => return self.error = Some(Error::Unsupported),
        };
        self.content = Some(content);
    }

    /// Pick up a worker result or retry after a superseded render releases the slot.
    fn poll(&mut self, ctx: &egui::Context, host: Option<&Host>) {
        let now = ctx.input(|i| i.time);
        if let Some(at) = self.retry_at {
            if now < at {
                ctx.request_repaint_after_for(Duration::from_secs_f64(at - now), self.viewport);
                return;
            }
            self.retry_at = None;
            match host {
                Some(host) => self.submit(host),
                None => self.error = Some(Error::Unavailable),
            }
        }
        let Some(pending) = &self.pending else { return };
        let result = match pending.try_recv() {
            Ok(value) => value,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(Error::Failed),
        };
        ctx.request_repaint_of(self.viewport);
        self.pending = None;
        match result {
            Ok(page) if (self.page..=MAX_PAGES).contains(&page.pages) => {
                self.pages = Some(page.pages);
                self.texture = Some(ctx.load_texture("attachment-preview", page.image, egui::TextureOptions::LINEAR));
            }
            Ok(_) => self.error = Some(Error::Invalid),
            Err(Error::Busy) if self.retries < MAX_RETRIES => {
                self.retries += 1;
                self.retry_at = Some(now + RETRY_DELAY.as_secs_f64());
                ctx.request_repaint_after_for(RETRY_DELAY, self.viewport);
            }
            Err(error) => self.error = Some(error),
        }
    }
}

/// Text from a file's first `limit` bytes, and whether there was more: UTF-16 by its byte order
/// mark, else UTF-8 when it is (cut where a character ends), else Windows-1252.
pub fn text_of(bytes: &[u8], limit: usize) -> (String, bool) {
    let cut = bytes.len() > limit;
    let mut head = &bytes[..bytes.len().min(limit)];
    if head.starts_with(&[0xff, 0xfe]) || head.starts_with(&[0xfe, 0xff]) {
        let label = if head[0] == 0xff { "utf-16le" } else { "utf-16be" };
        let even = head.len() & !1;
        return (mailcraft_charset::decode(&head[..even], label), cut);
    }
    // (Only a file cut here can end inside a character: a whole file's last byte is text.)
    if cut
        && let Err(error) = std::str::from_utf8(head)
        && error.error_len().is_none()
    {
        head = &head[..error.valid_up_to()];
    }
    let text = mailcraft_charset::guess(head);
    let text = text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text);
    (text, cut)
}

/// The rows of comma- (or `separator`-) separated values, quoted as RFC 4180 quotes them: at
/// most [`MAX_ROWS`] of [`MAX_COLUMNS`] cells of [`MAX_CELL`] characters, and whether there were more.
pub fn table_of(text: &str, separator: char) -> (Vec<Vec<String>>, bool) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut length = 0;
    let mut quoted = false;
    let mut more = false;
    let mut chars = text.chars().peekable();
    let push = |cell: &mut String, length: &mut usize, c: char| {
        if *length < MAX_CELL {
            cell.push(c);
        } else if *length == MAX_CELL {
            cell.push('…');
        }
        *length += 1;
    };
    while let Some(c) = chars.next() {
        if quoted {
            if c != '"' {
                push(&mut cell, &mut length, c);
            } else if chars.peek() == Some(&'"') {
                chars.next();
                push(&mut cell, &mut length, '"');
            } else {
                quoted = false;
            }
        } else if c == '"' && length == 0 {
            quoted = true;
        } else if c == separator || c == '\n' {
            if row.len() < MAX_COLUMNS {
                row.push(std::mem::take(&mut cell));
            } else {
                cell.clear();
                more = true;
            }
            length = 0;
            if c == '\n' {
                rows.push(std::mem::take(&mut row));
                if rows.len() == MAX_ROWS {
                    more |= chars.peek().is_some();
                    return (rows, more);
                }
            }
        } else if c != '\r' {
            push(&mut cell, &mut length, c);
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        if row.len() < MAX_COLUMNS {
            row.push(cell);
        }
        rows.push(row);
    }
    (rows, more)
}

/// The files a ZIP file lists in its central directory, by name and size (at most
/// [`MAX_ENTRIES`]), and how many it says it has. Nothing is unpacked. `None` for what isn't a
/// ZIP file this reads.
pub fn entries_of(bytes: &[u8]) -> Option<(Vec<(String, u64)>, usize)> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    // The end of central directory record, in the last 64 KiB and 22 bytes (its comment's room).
    let from = bytes.len().saturating_sub(22 + 65_535);
    let end = (from..=bytes.len().checked_sub(22)?).rev().find(|at| bytes[*at..].starts_with(b"PK\x05\x06"))?;
    let total = usize::from(u16_at(end + 10)?);
    let mut at = usize::try_from(u32_at(end + 16)?).ok()?;
    let mut entries = Vec::new();
    while entries.len() < total.min(MAX_ENTRIES) {
        if u32_at(at)? != 0x0201_4b50 {
            return None;
        }
        let size = u64::from(u32_at(at + 24)?);
        let name_len = usize::from(u16_at(at + 28)?);
        let extra = usize::from(u16_at(at + 30)?);
        let comment = usize::from(u16_at(at + 32)?);
        let name = bytes.get(at + 46..at + 46 + name_len)?;
        let name: String = mailcraft_charset::guess(name).chars().filter(|c| !c.is_control()).collect();
        entries.push((name, size));
        at += 46 + name_len + extra + comment;
    }
    Some((entries, total))
}

/// Release a preview or click drawn where `spot` of `viewport` now shows something else (`shown`).
pub(crate) fn follow(app: &mut MailApp, viewport: egui::ViewportId, shown: Option<Origin>) {
    let spot = app.hit.window();
    if app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && p.spot == spot && Some(p.origin) != shown) {
        app.attachment_preview = None;
    }
    if app.pending_attachment_preview.as_ref().is_some_and(|click| click.viewport == viewport && click.spot == spot && Some(click.origin) != shown) {
        cancel_click(app);
    }
}

/// The main window's selected message while the reading pane can show it.
pub(crate) fn pane_message(app: &MailApp) -> Option<Id> {
    (app.session.view.module == Module::Mail
        && app.session.view.reading_pane != Pane::Off
        && app.session.sel.messages.len() == 1
        && crate::windows::compose_window::pane_compose(app).is_none())
    .then(|| app.session.sel.focus())
    .flatten()
}

/// Drop stale attachments before polling the active preview.
pub(crate) fn poll(app: &mut MailApp, ctx: &egui::Context) {
    if app.pending_attachment_preview.is_none() && app.attachment_preview.is_none() {
        return;
    }
    let viewports = crate::windows::viewports(app);
    let windows = crate::windows::ids(app);
    let window_is_open = |viewport: egui::ViewportId, spot: u64| {
        (viewport == egui::ViewportId::ROOT || viewports.contains(&viewport)) && (spot == 0 || windows.contains(&spot))
    };
    if app
        .pending_attachment_preview
        .as_ref()
        .is_some_and(|click| click.mailbox != app.session.openings() || !window_is_open(click.viewport, click.spot))
    {
        cancel_click(app);
    }
    if app.attachment_preview.as_ref().is_some_and(|preview| {
        preview.mailbox != app.session.openings()
            || !window_is_open(preview.viewport, preview.spot)
            || attachment(app, preview.origin, preview.index) != Some(&preview.attachment)
            || !enabled(app)
    }) {
        app.attachment_preview = None;
    }
    if let Some(preview) = &mut app.attachment_preview {
        preview.poll(ctx, app.services.attachment_preview.as_ref());
    }
}

/// Draw the preview in place of a message's body when it belongs here and the Preview Pane is
/// off.
pub(crate) fn show(app: &mut MailApp, ui: &mut egui::Ui, id: Id) -> bool {
    show_in_place(app, ui, Origin::Message(id))
}

/// Draw the preview of a file of the message being written `compose` in place of its body, when
/// it belongs here and the Preview Pane is off.
pub(crate) fn show_draft(app: &mut MailApp, ui: &mut egui::Ui, compose: u64) -> bool {
    show_in_place(app, ui, Origin::Draft(compose))
}

fn show_in_place(app: &mut MailApp, ui: &mut egui::Ui, origin: Origin) -> bool {
    let (viewport, spot) = (ui.ctx().viewport_id(), app.hit.window());
    if app.session.view.preview_pane
        || !app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && p.spot == spot && p.origin == origin)
    {
        return false;
    }
    draw(app, ui, true);
    true
}

/// The Preview Pane on the right of `ui`, when it is on (`view.previewPane`), as panel `salt`:
/// as wide as the owner dragged it.
pub(crate) fn pane(app: &mut MailApp, ui: &mut egui::Ui, salt: egui::Id) {
    if !app.session.view.preview_pane {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let max = (ui.available_width() * 0.6).max(200.0);
    let shown = egui::Panel::right(salt)
        .default_size(app.ui.preview_pane_width.min(max))
        .size_range(200.0..=max)
        .resizable(true)
        .frame(egui::Frame::NONE.fill(t.surface).stroke(egui::Stroke::new(1.0, t.border)))
        .show(ui, |ui| show_pane(app, ui));
    app.ui.preview_pane_width = shown.response.rect.width();
}

/// The Preview Pane's contents: the preview shown where it is, or a word on how to show one.
fn show_pane(app: &mut MailApp, ui: &mut egui::Ui) {
    let (viewport, spot) = (ui.ctx().viewport_id(), app.hit.window());
    app.hit.add("attachmentPreviewPane", 0, ui.max_rect());
    if app.attachment_preview.as_ref().is_some_and(|p| p.viewport == viewport && p.spot == spot) {
        draw(app, ui, false);
        return;
    }
    let t = Tokens::get(ui.ctx());
    let r = ui.available_rect_before_wrap();
    let at = egui::pos2(r.center().x, r.min.y + r.height() * 0.38);
    crate::icons::paint(ui.painter(), egui::Rect::from_center_size(at, egui::vec2(40.0, 40.0)), "attach", t.border_strong, t.border_strong);
    ui.painter().text(
        at + egui::vec2(0.0, 40.0),
        egui::Align2::CENTER_CENTER,
        tl!("Select an attachment to preview it"),
        crate::theme::regular(13.0),
        t.text_dim,
    );
}

/// The preview: its name and size, its pages and zoom, and what it shows. `in_place`: in place of
/// the message's body, with the way back to it; else in the Preview Pane, with a way to close it.
fn draw(app: &mut MailApp, ui: &mut egui::Ui, in_place: bool) {
    let tokens = Tokens::get(ui.ctx());
    let Some(preview) = app.attachment_preview.as_mut() else { return };
    let key = preview.origin.key();
    let mut turn = None;
    let mut close = false;
    egui::Frame::NONE.fill(tokens.panel).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        ui.horizontal(|ui| {
            if in_place {
                let back = ui.button(tl!("Back to Message"));
                app.hit.add("attachmentPreviewBack", key, back.rect);
                close = back.clicked();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !in_place {
                    let x = crate::widgets::icon_button(ui, "close", tl!("Close Preview"), 14.0, true);
                    app.hit.add("attachmentPreviewClose", key, x.rect);
                    close |= x.clicked();
                }
                ui.label(egui::RichText::new(&preview.size).color(tokens.text_dim));
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&preview.name).strong()).truncate());
                });
            });
        });
        let pages = preview.pages.filter(|pages| *pages > 1);
        if pages.is_some() || preview.texture.is_some() {
            ui.horizontal_wrapped(|ui| {
                if let Some(pages) = pages {
                    let ready = preview.pending.is_none() && preview.retry_at.is_none();
                    if ui.add_enabled(ready && preview.page > 1, egui::Button::new(tl!("Previous Page"))).clicked() {
                        turn = Some(preview.page - 1);
                    }
                    ui.label(format!("{} / {pages}", preview.page));
                    let next = ui.add_enabled(ready && preview.page < pages, egui::Button::new(tl!("Next Page")));
                    app.hit.add("attachmentPreviewNext", key, next.rect);
                    if next.clicked() {
                        turn = Some(preview.page + 1);
                    }
                }
                if preview.texture.is_some() {
                    ui.add(egui::Slider::new(&mut preview.zoom, 1.0..=3.0).text(tl!("Zoom")));
                }
            });
        }
    });
    if close {
        app.attachment_preview = None;
        return;
    }
    if let (Some(page), Some(host)) = (turn, &app.services.attachment_preview) {
        preview.start(host, page);
    }
    // What is shown is copied out: showing a message or an HTML file needs the app.
    let (texture, content, error, zoom, index) = (preview.texture.clone(), preview.content.clone(), preview.error, preview.zoom, preview.index);
    let busy = preview.pending.is_some() || preview.retry_at.is_some();
    ui.separator();
    if let Some(error) = error {
        egui::Frame::NONE.inner_margin(12).show(ui, |ui| {
            ui.colored_label(if error == Error::Unsupported { tokens.text_dim } else { tokens.red }, error.text());
        });
        return;
    }
    if let Some(texture) = texture {
        show_picture(app, ui, &texture, zoom, key, index);
        return;
    }
    let Some(content) = content else {
        egui::Frame::NONE.inner_margin(12).show(ui, |ui| {
            ui.horizontal(|ui| {
                if busy {
                    crate::software::spinner(ui, ui.spacing().interact_size.y, None);
                }
                ui.label(tl!("Preparing attachment preview…"));
            });
        });
        return;
    };
    // One scroll state for what is shown, at the top again for another attachment.
    let scroll = egui::Id::unique(("attachment-preview-scroll", key, index));
    match content {
        Content::Message(m) => crate::reading::show_file_message(app, ui, &m, Place::Attachment),
        Content::Html(m) => crate::reading::show_attachment_html(app, ui, &m),
        Content::Text { text, cut, mono } => {
            note(ui, cut.then(|| tl!("Showing the start of the file.").to_string()));
            egui::ScrollArea::both().auto_shrink([false, false]).id_salt(scroll).show(ui, |ui| {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    let mut shown: &str = &text;
                    let mut edit = egui::TextEdit::multiline(&mut shown).frame(egui::Frame::NONE).desired_width(f32::INFINITY);
                    if mono {
                        edit = edit.font(egui::TextStyle::Monospace);
                    }
                    let response = ui.add(edit);
                    app.hit.add("attachmentPreviewText", key, response.rect);
                });
            });
        }
        Content::Table { rows, cut } => {
            note(ui, cut.then(|| crate::i18n::fmt(tl!("Showing the first {n} rows."), &[("n", &MAX_ROWS.to_string())])));
            egui::ScrollArea::both().auto_shrink([false, false]).id_salt(scroll).show(ui, |ui| {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    let grid =
                        egui::Grid::new(("attachment-preview-table", key, index)).striped(true).spacing(egui::vec2(16.0, 4.0)).show(ui, |ui| {
                            for (n, row) in rows.iter().enumerate() {
                                for cell in row {
                                    let text = egui::RichText::new(cell);
                                    ui.label(if n == 0 { text.strong() } else { text });
                                }
                                ui.end_row();
                            }
                        });
                    app.hit.add("attachmentPreviewTable", key, grid.response.rect);
                });
            });
        }
        Content::Entries { entries, total } => {
            let said = tln!(total, "{n} file", "{n} files");
            note(ui, Some(said));
            egui::ScrollArea::both().auto_shrink([false, false]).id_salt(scroll).show(ui, |ui| {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    let grid =
                        egui::Grid::new(("attachment-preview-files", key, index)).striped(true).spacing(egui::vec2(16.0, 4.0)).show(ui, |ui| {
                            for (name, size) in entries.iter() {
                                ui.label(name);
                                ui.label(egui::RichText::new(mailcraft_model::size_label_in(crate::i18n::current(), *size)).color(tokens.text_dim));
                                ui.end_row();
                            }
                        });
                    app.hit.add("attachmentPreviewFiles", key, grid.response.rect);
                    if total > entries.len() {
                        let more = total - entries.len();
                        ui.label(egui::RichText::new(tln!(more, "and {n} more", "and {n} more")).color(tokens.text_dim));
                    }
                });
            });
        }
    }
}

/// A short line over what is shown (how much of it is).
fn note(ui: &mut egui::Ui, said: Option<String>) {
    let Some(said) = said else { return };
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 4 }).show(ui, |ui| {
        ui.label(egui::RichText::new(said).font(crate::theme::regular(11.5)).color(t.text_dim));
    });
}

/// A page or picture, fitted to the room and zoomed.
fn show_picture(app: &mut MailApp, ui: &mut egui::Ui, texture: &egui::TextureHandle, zoom: f32, key: u64, index: usize) {
    let tokens = Tokens::get(ui.ctx());
    let size = texture.size_vec2();
    let room = ui.available_size().max(egui::vec2(1.0, 1.0));
    let display = size * (room.x / size.x).min(room.y / size.y).min(1.0) * zoom;
    egui::Frame::NONE.fill(tokens.canvas).show(ui, |ui| {
        // One scroll state for the preview, at the top again for another attachment: not one
        // in egui's memory for every attachment ever previewed.
        let mut area = egui::ScrollArea::both().auto_shrink([false, false]).id_salt("attachment-preview");
        let shown = egui::Id::unique(("mailcraft-attachment-preview-shown", ui.ctx().viewport_id(), app.hit.window()));
        if ui.ctx().data(|d| d.get_temp::<(u64, usize)>(shown)) != Some((key, index)) {
            ui.ctx().data_mut(|d| d.insert_temp(shown, (key, index)));
            area = area.scroll_offset(egui::Vec2::ZERO);
        }
        area.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(((room.x - display.x) / 2.0).max(0.0));
                let image = ui.add(egui::Image::new((texture.id(), display)));
                app.hit.add("attachmentPreviewImage", key, image.rect);
            });
        });
    });
}

#[cfg(test)]
#[path = "attachment_preview_tests.rs"]
mod tests;

/// The attachment of message `message` previewed in window `window` (0: the main window), if any:
/// what the Attachments tab of the ribbon acts on (#1478).
pub(crate) fn target(app: &MailApp, window: u64, message: Id) -> Option<usize> {
    app.attachment_preview.as_ref().filter(|p| p.spot == window && p.origin == Origin::Message(message)).map(|p| p.index)
}

/// The main window's message and its previewed attachment, while the reading pane shows it.
pub(crate) fn pane_target(app: &MailApp) -> Option<(Id, usize)> {
    let id = pane_message(app)?;
    Some((id, target(app, 0, id)?))
}

/// Release the preview of window `window` when it starts showing another message (`id`).
pub(crate) fn follow_message(app: &mut MailApp, window: u64, id: Option<Id>) {
    let shown = id.map(Origin::Message);
    if app.attachment_preview.as_ref().is_some_and(|p| p.spot == window && matches!(p.origin, Origin::Message(_)) && Some(p.origin) != shown) {
        app.attachment_preview = None;
    }
    if app
        .pending_attachment_preview
        .as_ref()
        .is_some_and(|click| click.spot == window && matches!(click.origin, Origin::Message(_)) && Some(click.origin) != shown)
    {
        cancel_click(app);
    }
}

/// Show Message is useful only while a preview hides this window's message body (the Preview
/// Pane off).
pub(crate) fn can_show_message(app: &MailApp) -> bool {
    !app.session.view.preview_pane && app.attachment_preview.as_ref().is_some_and(|p| matches!(p.origin, Origin::Message(_)))
}

/// Close the preview and any click waiting to start one.
pub(crate) fn close(app: &mut MailApp) {
    cancel_click(app);
    app.attachment_preview = None;
}
