// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A page of an attachment drawn for its preview (v0.12.14), by the Craft content runtime that
//! comes with MailCraft: the same verified runtime and cores the content index reads attachments
//! with ([`crate::content_index`]). One page, one worker, at most [`MAX_WIDTH`] pixels wide. The
//! attachment is written into a private folder of its own for the worker and goes with it;
//! nothing runs but the runtime's own verified cores, and nothing goes to the network.

use std::sync::atomic::AtomicBool;

/// The files the runtime's cores draw pages of, by extension (Word, Grid, Deck, PDF, Photo,
/// Vector and Fig Craft).
pub const EXTENSIONS: &[&str] =
    &["pdf", "docx", "docm", "doc", "odt", "xlsx", "xlsm", "xls", "xlsb", "ods", "pptx", "psd", "psb", "svg", "svgz", "ai", "fig"];

/// The widest a page is drawn, in pixels.
pub const MAX_WIDTH: u32 = 2048;

/// Most pages a document is said to have.
pub const MAX_PAGES: u32 = 10_000;

/// A page drawn: its pixels (RGBA, row by row) and how many pages the document has.
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub pages: u32,
}

/// Whether the runtime is installed beside the program (it is opened, and verified, only when a
/// page is first asked for).
pub fn installed() -> bool {
    #[cfg(windows)]
    {
        native::root().is_some_and(|root| root.join("runtime.json").is_file())
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Page `page` (from 1) of the attachment `bytes`, a file of type `extension` (one of
/// [`EXTENSIONS`]), drawn at most `width` pixels wide. `cancel` set ends the worker.
pub fn render(bytes: &[u8], extension: &str, page: u32, width: u32, cancel: &AtomicBool) -> Result<Page, String> {
    #[cfg(windows)]
    {
        native::render(bytes, extension, page, width, cancel)
    }
    #[cfg(not(windows))]
    {
        let _ = (bytes, extension, page, width, cancel);
        Err("the Craft content runtime is not available on this platform".into())
    }
}

#[cfg(windows)]
mod native {
    use std::fs::File;
    use std::io::{Cursor, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use craft_content::process::Cancellation;
    use craft_content::protocol::Rendered;
    use craft_content::runtime::{Client, Core, Runtime};
    use serde_json::Value;

    use super::{EXTENSIONS, MAX_PAGES, MAX_WIDTH, Page};

    type Result<T> = std::result::Result<T, String>;

    /// Most bytes of a page's PNG read back from the worker.
    const MAX_PNG: u64 = 64 * 1024 * 1024;

    /// Where the runtime is: `craft-content` beside the program, as for the content index.
    pub(super) fn root() -> Option<PathBuf> {
        std::env::current_exe().ok()?.parent().map(|dir| dir.join("craft-content"))
    }

    /// The runtime, opened and verified once.
    fn runtime() -> Result<&'static Runtime> {
        static RUNTIME: OnceLock<Result<Runtime>> = OnceLock::new();
        RUNTIME
            .get_or_init(|| root().ok_or_else(|| "the program has no folder".to_string()).and_then(|root| Runtime::open(&root)))
            .as_ref()
            .map_err(Clone::clone)
    }

    pub(super) fn render(bytes: &[u8], extension: &str, page: u32, width: u32, cancel: &AtomicBool) -> Result<Page> {
        if !EXTENSIONS.contains(&extension) {
            return Err("no Craft core draws this type of file".into());
        }
        if page == 0 || page > MAX_PAGES {
            return Err("no such page".into());
        }
        if bytes.len() as u64 > craft_content::MAX_INPUT {
            return Err("resourceLimit: the attachment exceeds the 64 MiB input allowance".into());
        }
        // Office documents are ZIP containers: one that declares an implausible expansion is not
        // handed to a core, as the content index doesn't.
        if let Some(why) = crate::content_index::zip_bomb(bytes) {
            return Err(why);
        }
        let runtime = runtime()?;
        let core = runtime.core(extension).ok_or("no selected Craft core draws this type of file")?;
        let scratch = tempfile::Builder::new().prefix("mailcraft-preview-").tempdir().map_err(|why| why.to_string())?;
        let cancellation = Cancellation::default();
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            // The owner closing the preview, or asking for another, ends the worker's process tree.
            scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    if cancel.load(Ordering::Relaxed) {
                        cancellation.cancel();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            });
            let result = draw(runtime, core, bytes, extension, page, width.clamp(1, MAX_WIDTH), scratch.path(), &cancellation);
            done.store(true, Ordering::Release);
            result
        })
    }

    fn draw(
        runtime: &Runtime,
        core: &Core,
        bytes: &[u8],
        extension: &str,
        page: u32,
        width: u32,
        scratch: &Path,
        cancellation: &Cancellation,
    ) -> Result<Page> {
        let input = scratch.join(format!("attachment.{extension}"));
        let mut file = File::create_new(&input).map_err(|why| why.to_string())?;
        file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|why| why.to_string())?;
        drop(file);
        let sha256 = craft_content::runtime::hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref());
        let mut worker = runtime.start(core, scratch, cancellation)?;
        let info = worker.open(&input, &sha256, extension, scratch)?;
        let (part, at, pages) = locate(&info, page)?;
        let rendered = worker.render_at(part, at, width)?;
        checked(&rendered, scratch)?;
        let (width_px, height_px, rgba) = match rendered.kind.as_str() {
            "png" => decode(&rendered.path, rendered.bytes)?,
            // A worksheet is drawn as its print document: its page is drawn from that.
            "pdf" if extension != "pdf" => {
                let pdf = runtime.core("pdf").ok_or("no selected Craft core draws the worksheet's print document")?;
                let mut printer = runtime.start(pdf, scratch, cancellation)?;
                let sheet = printer.open(&rendered.path, &rendered.sha256, "pdf", scratch)?;
                let (_, _, total) = locate(&sheet, 1)?;
                let shown = picture(&mut printer, at.min(total.saturating_sub(1)), width, scratch)?;
                let _ = printer.finish();
                shown
            }
            _ => return Err("the Craft worker returned an undeclared page representation".into()),
        };
        // Native warnings after a good close are not the owner's concern here.
        let _ = worker.finish();
        Ok(Page { width: width_px, height: height_px, rgba, pages })
    }

    /// Page `at` (from 0) of the first part of an open PDF.
    fn picture(worker: &mut Client, at: u32, width: u32, scratch: &Path) -> Result<(u32, u32, Vec<u8>)> {
        let rendered = worker.render_at(0, at, width)?;
        checked(&rendered, scratch)?;
        if rendered.kind != "png" {
            return Err("the PDF core returned no pixels".into());
        }
        decode(&rendered.path, rendered.bytes)
    }

    /// The part and its page (each from 0) that page `page` (from 1) of the whole document is,
    /// and how many pages the document has.
    pub(super) fn locate(info: &Value, page: u32) -> Result<(u32, u32, u32)> {
        let parts = info.get("parts").and_then(Value::as_array).ok_or("the Craft core returned no page inventory")?;
        let mut total = 0u64;
        let mut found = None;
        for (index, part) in parts.iter().enumerate() {
            let pages = part.get("pages").and_then(Value::as_u64).ok_or("the Craft core returned an invalid page inventory")?;
            if found.is_none() && u64::from(page) > total && u64::from(page) <= total.saturating_add(pages) {
                found = Some((index, u64::from(page) - total - 1));
            }
            total = total.saturating_add(pages);
        }
        let total = u32::try_from(total.min(u64::from(MAX_PAGES))).map_err(|why| why.to_string())?;
        let (part, at) = found.ok_or("the document has no such page")?;
        Ok((u32::try_from(part).map_err(|why| why.to_string())?, u32::try_from(at).map_err(|why| why.to_string())?, total))
    }

    /// The page is the worker's own file in the private folder, as the worker said it is.
    fn checked(rendered: &Rendered, scratch: &Path) -> Result<()> {
        if rendered.path.parent().and_then(|path| path.canonicalize().ok()) != scratch.canonicalize().ok() {
            return Err("the Craft worker returned a page outside its private folder".into());
        }
        craft_content::runtime::verify(&rendered.path, &rendered.sha256, rendered.bytes)
    }

    /// A page's PNG, within bounds of bytes, size and memory.
    fn decode(path: &Path, bytes: u64) -> Result<(u32, u32, Vec<u8>)> {
        if bytes > MAX_PNG {
            return Err("resourceLimit: the drawn page is too large".into());
        }
        let mut data = Vec::new();
        File::open(path).and_then(|file| file.take(MAX_PNG + 1).read_to_end(&mut data)).map_err(|why| why.to_string())?;
        if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("the drawn page is not a PNG".into());
        }
        let mut reader = image::ImageReader::with_format(Cursor::new(data), image::ImageFormat::Png);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(4096);
        limits.max_image_height = Some(16_384);
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        let rgba = reader.decode().map_err(|why| why.to_string())?.into_rgba8();
        Ok((rgba.width(), rgba.height(), rgba.into_raw()))
    }
}

#[cfg(all(test, windows))]
#[path = "attachment_pages_tests.rs"]
mod tests;
