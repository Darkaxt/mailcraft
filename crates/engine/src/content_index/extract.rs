// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Full attachment text, finite Craft imports and local OCR. Every exclusion
//! belongs to this attachment's coverage instead of disappearing from search.

use std::io::{Cursor, Read};
#[cfg(windows)]
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use craft_content::Record;
use serde_json::{Value, json};

use super::{
    documents,
    input::{Input, hex, reference},
    segments::Spool,
};
use crate::Part;

type Result<T> = std::result::Result<T, String>;
const DECODED_ALLOWANCE: u64 = 128 * 1024 * 1024;
const NESTED_MESSAGES: usize = 8;
const RENDERED_PAGES: u64 = 512;
const EXTRACTED_TEXT: u64 = 256 * 1024 * 1024;

#[derive(Default)]
struct Coverage {
    gaps: Vec<Value>,
    sources: Vec<Value>,
    decoded: u64,
    rendered: u64,
    text_bytes: u64,
}

/// Deterministic allowance/deadline failures are resourceLimit gaps: they are
/// reported coverage, not retried as transient failures at every launch.
#[cfg(windows)]
pub(super) fn failure(why: &str) -> &'static str {
    if why.contains("resourceLimit:") { "resourceLimit" } else { "failed" }
}

impl Coverage {
    fn gap(&mut self, location: &Value, code: &str, detail: impl Into<String>) {
        self.gaps.push(json!({"location":location,"code":code,"detail":detail.into()}));
    }
}

pub struct Extractor {
    revision: String,
    #[cfg(windows)]
    runtime: std::result::Result<craft_content::runtime::Runtime, String>,
    #[cfg(windows)]
    pub cancellation: craft_content::process::Cancellation,
    #[cfg(windows)]
    evidence: Option<super::evidence::Evidence>,
}

impl Extractor {
    pub fn new(evidence: Option<PathBuf>) -> Result<Self> {
        #[cfg(windows)]
        {
            let runtime = std::env::current_exe().map_err(|why| why.to_string()).and_then(|executable| {
                executable
                    .parent()
                    .ok_or_else(|| "the app package has no parent directory".into())
                    .and_then(|parent| craft_content::runtime::Runtime::open(&parent.join("craft-content")))
            });
            let revision = runtime.as_ref().map(|runtime| runtime.revision.clone()).unwrap_or_else(|_| "native-runtime-unavailable-1".into());
            let evidence = evidence.map(super::evidence::Evidence::new).transpose()?;
            Ok(Self { revision, runtime, cancellation: Default::default(), evidence })
        }
        #[cfg(not(windows))]
        {
            if evidence.is_some() {
                return Err("native render evidence is not available on this platform".into());
            }
            Ok(Self { revision: "native-worker-platform-unavailable-1".into() })
        }
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn attachment(&self, input: &Input, index: usize, scratch: &Path, spool: &mut Spool, cancelled: &AtomicBool) -> Result<(bool, Value)> {
        let file = input.message.attachments.get(index).ok_or("the attachment no longer exists")?;
        let content = reference(&input.message, Part::Attachment(index))?;
        let location = json!({"attachment":index});
        let mut coverage = Coverage::default();
        if content.len > craft_content::MAX_INPUT {
            coverage.gap(&location, "resourceLimit", "attachment exceeds the explicit 64 MiB native/decoded-input allowance");
        } else if !input.metadata.downloaded {
            coverage.gap(&location, "missing", "the complete message or attachment has not been downloaded");
        } else {
            let mut bytes = Vec::new();
            input.open(Part::Attachment(index))?.take(craft_content::MAX_INPUT + 1).read_to_end(&mut bytes).map_err(|why| why.to_string())?;
            if bytes.len() as u64 != content.len {
                return Err("the original attachment stream changed length before its verified EOF".into());
            }
            let actual = hex(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref());
            if actual != hex(&content.sha256) {
                return Err("the original attachment does not match its independently retained SHA-256".into());
            }
            self.bytes(&bytes, &file.name, &file.mime, "", location, 0, scratch, spool, cancelled, &mut coverage)?;
        }
        let complete = coverage.gaps.is_empty();
        Ok((
            complete,
            json!({"complete":complete,"gaps":coverage.gaps,"sources":coverage.sources,
            "decodedBytes":coverage.decoded,"renderedPages":coverage.rendered,
            "limits":{"inputBytes":craft_content::MAX_INPUT,"decodedBytes":DECODED_ALLOWANCE,
                "nestedMessages":NESTED_MESSAGES,"renderedPages":RENDERED_PAGES,"extractedTextBytes":EXTRACTED_TEXT},
            "ocrAccuracy":"Machine recognition with reported language models and confidence; a completed pass is not a guarantee that every glyph was recognized."}),
        ))
    }

    fn bytes(
        &self,
        bytes: &[u8],
        name: &str,
        mime: &str,
        charset: &str,
        location: Value,
        depth: usize,
        scratch: &Path,
        spool: &mut Spool,
        cancelled: &AtomicBool,
        coverage: &mut Coverage,
    ) -> Result<()> {
        if cancelled.load(Ordering::Acquire) {
            return Err("indexing cancelled".into());
        }
        let decoded = coverage.decoded.checked_add(bytes.len() as u64).ok_or("attachment decoded-byte count overflow")?;
        if bytes.len() as u64 > craft_content::MAX_INPUT || decoded > DECODED_ALLOWANCE {
            coverage.gap(&location, "resourceLimit", "nested attachment exceeds its 64 MiB input or 128 MiB cumulative decoded allowance");
            return Ok(());
        }
        coverage.decoded = decoded;
        let mime = mime.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        let extension = extension(name, &mime, bytes);
        if matches!(extension.as_str(), "eml" | "rfc822") || mime == "message/rfc822" {
            if depth >= NESTED_MESSAGES {
                coverage.gap(&location, "resourceLimit", "attached-message nesting exceeds eight messages");
                return Ok(());
            }
            let report = mailcraft_mime::content::visit(bytes, DECODED_ALLOWANCE.saturating_sub(coverage.decoded), |leaf| {
                let unit = json!({"source":location,"mimePart":leaf.path});
                let header = mailcraft_charset::guess(leaf.header);
                spool.text(Cursor::new(header.as_bytes()), json!({"source":unit,"field":"mimeHeaders"}), "native", cancelled)?;
                self.bytes(&leaf.decoded, &leaf.name, &leaf.mime, &leaf.charset, unit, depth.saturating_add(1), scratch, spool, cancelled, coverage)
            })?;
            for gap in report.gaps {
                coverage.gap(&json!({"source":location,"mimePart":gap.path}), gap.code, gap.detail);
            }
            coverage
                .sources
                .push(json!({"location":location,"engine":"RFC 5322/MIME leaf inspection","parts":report.visited,"leaves":report.leaves}));
        } else if mime.starts_with("text/")
            || matches!(mime.as_str(), "application/json" | "application/xml" | "application/javascript")
            || matches!(
                extension.as_str(),
                "txt" | "csv" | "tsv" | "md" | "log" | "json" | "xml" | "yaml" | "yml" | "ics" | "vcf" | "html" | "htm" | "css" | "js"
            )
        {
            let text = mailcraft_charset::decode(bytes, charset);
            if coverage.text_bytes.checked_add(text.len() as u64).is_none_or(|count| count > EXTRACTED_TEXT) {
                coverage.gap(&location, "resourceLimit", "decoded text exceeds the 256 MiB containing-attachment text allowance");
                return Ok(());
            }
            coverage.text_bytes += text.len() as u64;
            if charset.is_empty() && std::str::from_utf8(bytes).is_err() {
                coverage.gap(
                    &location,
                    "unsupported",
                    "attachment declares no retained charset and is not UTF-8; Windows-1252 text is indexed as an explicit best-effort decoding",
                );
            }
            spool.text(Cursor::new(text.as_bytes()), json!({"source":location,"representation":"decodedSource"}), "native", cancelled)?;
            if mime == "text/html" || matches!(extension.as_str(), "html" | "htm") {
                let (_, errors) = documents::html(
                    spool,
                    Cursor::new(text.as_bytes()),
                    json!({"source":location,"representation":"htmlText","hiddenTextIncluded":true}),
                    cancelled,
                )?;
                coverage.sources.push(json!({"location":location,"engine":"HTML5 text tokenizer","htmlRecoveryEvents":errors,"charset":charset}));
            } else {
                coverage.sources.push(json!({"location":location,"engine":"decoded text","charset":charset}));
            }
        } else {
            #[cfg(windows)]
            match &self.runtime {
                Ok(runtime) if runtime.core(&extension).is_some() => {
                    let mut file = tempfile::NamedTempFile::new_in(scratch).map_err(|why| why.to_string())?;
                    file.write_all(bytes).and_then(|_| file.flush()).and_then(|_| file.as_file().sync_all()).map_err(|why| why.to_string())?;
                    let sha256 = hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref());
                    self.native(runtime, file.path(), &sha256, &extension, location, scratch, spool, cancelled, coverage)?;
                }
                Ok(runtime) if mime.starts_with("image/") || image::guess_format(bytes).is_ok() => {
                    self.image(runtime, bytes, location, scratch, spool, coverage)?;
                }
                Ok(_) => coverage.gap(&location, "unsupported", format!("no supported native text/OCR importer for {mime} / {extension}")),
                Err(why) => coverage.gap(&location, "unsupported", format!("the selected Craft content runtime is unavailable: {why}")),
            }
            #[cfg(not(windows))]
            {
                let _ = scratch;
                coverage.gap(
                    &location,
                    "unsupported",
                    format!("this package has no qualified native core/OCR worker for {mime} / {extension} on this platform"),
                );
            }
        }
        Ok(())
    }

    #[cfg(windows)]
    fn native(
        &self,
        runtime: &craft_content::runtime::Runtime,
        input: &Path,
        sha256: &str,
        extension: &str,
        location: Value,
        scratch: &Path,
        spool: &mut Spool,
        cancelled: &AtomicBool,
        coverage: &mut Coverage,
    ) -> Result<()> {
        let core = runtime.core(extension).ok_or("no selected native importer matches the attachment")?;
        let mut worker = match runtime.start(core, scratch, &self.cancellation) {
            Ok(worker) => worker,
            Err(why) => {
                coverage.gap(&location, failure(&why), why);
                return Ok(());
            }
        };
        let info = match worker.open(input, sha256, extension, scratch) {
            Ok(info) => info,
            Err(why) => {
                coverage.gap(&location, failure(&why), why);
                return Ok(());
            }
        };
        if let Some(notes) = info.get("notes").and_then(Value::as_array) {
            for note in notes.iter().filter_map(Value::as_str).filter(|note| !note.trim().is_empty()) {
                coverage.gap(&location, "unsupported", note);
            }
        }
        let mut cursor = craft_content::Cursor { unit: 0, offset: 0 };
        let mut page_text = Spool::new(scratch)?;
        let mut text_page = None::<u64>;
        let mut page_offset = 0u64;
        let mut previous_unit = None::<Value>;
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err("indexing cancelled".into());
            }
            let batch = match worker.extract(cursor.clone()) {
                Ok(batch) => batch,
                Err(why) => {
                    coverage.gap(&location, failure(&why), why);
                    return Ok(());
                }
            };
            for mut record in batch.records {
                if extension == "pdf" {
                    let page = record.location.get("page").and_then(Value::as_u64).ok_or("the native PDF text record has no page identity")?;
                    if text_page != Some(page) {
                        text_page = Some(page);
                        page_offset = 0;
                        previous_unit = None;
                    }
                    let mut text = String::new();
                    if previous_unit.as_ref().is_some_and(|unit| unit != &record.location) {
                        text.push('\n')
                    }
                    text.push_str(&record.text);
                    let bytes = text.len() as u64;
                    page_text.record(Record {
                        location: json!({"source":location,"representation":"canonicalPdfPageText","page":page,
                        "betweenImportedLines":"LF; visual line wrapping is not silently changed into spaces"}),
                        offset: page_offset,
                        text,
                        derivation: record.derivation.clone(),
                    })?;
                    page_offset = page_offset.checked_add(bytes).ok_or("canonical PDF page text offset overflow")?;
                    previous_unit = Some(record.location.clone());
                }
                record.location = json!({"source":location,"native":record.location});
                if let Err(why) = located(spool, record, coverage) {
                    coverage.gap(&location, "resourceLimit", why);
                    return Ok(());
                }
            }
            for gap in batch.gaps {
                coverage.gap(&json!({"source":location,"native":gap.location}), &gap.code, gap.detail);
            }
            let source = json!({"location":location,"engine":core.id,"source":core.source,"coverage":batch.coverage});
            if !coverage.sources.contains(&source) {
                coverage.sources.push(source)
            }
            let Some(next) = batch.next else { break };
            if (next.unit, next.offset) <= (cursor.unit, cursor.offset) {
                coverage.gap(&location, "failed", "the original native text cursor did not advance; it was not replayed");
                return Ok(());
            }
            cursor = next;
        }
        if extension == "pdf" {
            spool.append(&mut page_text)?
        }
        let parts = info.get("parts").and_then(Value::as_array).ok_or("the native core returned no part/page inventory")?;
        for (part, description) in parts.iter().enumerate() {
            let pages = description.get("pages").and_then(Value::as_u64).ok_or("the native core returned an invalid page inventory")?;
            for page in 0..pages {
                if cancelled.load(Ordering::Acquire) {
                    return Err("indexing cancelled".into());
                }
                if coverage.rendered >= RENDERED_PAGES {
                    coverage.gap(&json!({"source":location,"part":part,"page":page}),"resourceLimit",
                        format!("OCR stopped at the explicit {RENDERED_PAGES}-page attachment allowance; native text still has its separately reported coverage"));
                    break;
                }
                let rendered =
                    match worker.render(u32::try_from(part).map_err(|why| why.to_string())?, u32::try_from(page).map_err(|why| why.to_string())?) {
                        Ok(rendered) => rendered,
                        Err(why) => {
                            coverage.gap(&json!({"source":location,"part":part,"page":page}), failure(&why), why);
                            return Ok(());
                        }
                    };
                if rendered.path.parent().and_then(|path| path.canonicalize().ok()) != scratch.canonicalize().ok() {
                    coverage.gap(&location, "failed", "the native core returned a page outside its exact scratch root");
                    return Ok(());
                }
                craft_content::runtime::verify(&rendered.path, &rendered.sha256, rendered.bytes)?;
                if rendered.kind == "pdf" {
                    if extension == "pdf" {
                        coverage.gap(&location, "failed", "the PDF renderer returned a nested PDF instead of pixels");
                    } else if rendered.bytes > craft_content::MAX_INPUT {
                        coverage.gap(&location, "resourceLimit", "the worksheet print PDF exceeds the 64 MiB native import allowance");
                    } else {
                        self.native(
                            runtime,
                            &rendered.path,
                            &rendered.sha256,
                            "pdf",
                            json!({"source":location,"sheet":part,"representation":"printPdf"}),
                            scratch,
                            spool,
                            cancelled,
                            coverage,
                        )?;
                    }
                    // Grid's print document contains every page of this sheet;
                    // requesting it again for page 1 would duplicate extraction.
                    break;
                } else if rendered.kind == "png" {
                    coverage.rendered += 1;
                    let source = json!({"source":location,"part":part,"page":page,"width":rendered.width,"height":rendered.height});
                    if let Some(evidence) = &self.evidence {
                        evidence.capture(
                            &rendered.path,
                            &rendered.sha256,
                            rendered.bytes,
                            json!({"engine":core.id,"inputSha256":sha256,"source":source,"runtimeRevision":runtime.revision}),
                        )?;
                    }
                    self.recognize(runtime, &rendered.path, &rendered.sha256, rendered.bytes, scratch, source, spool, coverage)?;
                } else {
                    coverage.gap(&location, "failed", "the native worker returned an undeclared render representation");
                }
            }
        }
        match worker.finish() {
            // Native stderr after a successful close is a reported note only.
            Ok(warnings) if !warnings.is_empty() => coverage.sources.push(json!({"location":location,"nativeWarnings":warnings})),
            Ok(_) => {}
            Err(why) => coverage.gap(&location, failure(&why), why),
        }
        Ok(())
    }

    #[cfg(windows)]
    fn image(
        &self,
        runtime: &craft_content::runtime::Runtime,
        bytes: &[u8],
        location: Value,
        scratch: &Path,
        spool: &mut Spool,
        coverage: &mut Coverage,
    ) -> Result<()> {
        use image::ImageEncoder;
        if coverage.rendered >= RENDERED_PAGES {
            coverage.gap(&location, "resourceLimit", "image OCR exceeds the 512-page/image containing-attachment allowance");
            return Ok(());
        }
        let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|why| why.to_string())?;
        let format = reader.format();
        let (width, height) = reader.into_dimensions().map_err(|why| why.to_string())?;
        if width == 0 || height == 0 || u64::from(width).checked_mul(u64::from(height)).is_none_or(|pixels| pixels > craft_content::MAX_PIXELS) {
            coverage.gap(&location, "resourceLimit", "image exceeds its decoded 64-megapixel allowance");
            return Ok(());
        }
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|why| why.to_string())?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(craft_content::MAX_DECODED);
        reader.limits(limits);
        let picture = reader.decode().map_err(|why| why.to_string())?;
        let picture = if width.max(height) > 3000 {
            let scaled = picture.thumbnail(3000, 3000);
            drop(picture);
            scaled
        } else {
            picture
        };
        let mut pixels = picture.into_rgba8();
        for pixel in pixels.pixels_mut() {
            let alpha = u16::from(pixel.0.get(3).copied().unwrap_or(255));
            for channel in pixel.0.iter_mut().take(3) {
                *channel = ((u16::from(*channel) * alpha + 255 * (255 - alpha)) / 255) as u8;
            }
            if let Some(alpha) = pixel.0.get_mut(3) {
                *alpha = 255
            }
        }
        let mut file = tempfile::NamedTempFile::new_in(scratch).map_err(|why| why.to_string())?;
        image::codecs::png::PngEncoder::new(file.as_file_mut())
            .write_image(pixels.as_raw(), pixels.width(), pixels.height(), image::ExtendedColorType::Rgba8)
            .map_err(|why| why.to_string())?;
        file.flush().and_then(|_| file.as_file().sync_all()).map_err(|why| why.to_string())?;
        let length = file.as_file().metadata().map_err(|why| why.to_string())?.len();
        file.seek(SeekFrom::Start(0)).map_err(|why| why.to_string())?;
        let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = [0u8; 32 * 1024];
        loop {
            let count = file.read(&mut buffer).map_err(|why| why.to_string())?;
            if count == 0 {
                break;
            }
            hash.update(buffer.get(..count).ok_or("invalid OCR image read")?);
        }
        let sha256 = hex(hash.finish().as_ref());
        let source =
            json!({"source":location,"image":0,"width":pixels.width(),"height":pixels.height(),"originalWidth":width,"originalHeight":height});
        if let Some(evidence) = &self.evidence {
            evidence.capture(
                file.path(),
                &sha256,
                length,
                json!({"engine":"raster OCR input","source":source,"runtimeRevision":runtime.revision}),
            )?;
        }
        self.recognize(runtime, file.path(), &sha256, length, scratch, source, spool, coverage)?;
        if matches!(format, Some(image::ImageFormat::Gif | image::ImageFormat::WebP | image::ImageFormat::Tiff)) {
            coverage.gap(
                &location,
                "unsupported",
                "only the first image/frame is decoded for GIF, WebP and TIFF; additional animation frames/pages are not inferred",
            );
        }
        coverage.rendered += 1;
        Ok(())
    }

    #[cfg(windows)]
    fn recognize(
        &self,
        runtime: &craft_content::runtime::Runtime,
        image: &Path,
        sha256: &str,
        bytes: u64,
        scratch: &Path,
        source: Value,
        spool: &mut Spool,
        coverage: &mut Coverage,
    ) -> Result<()> {
        // Keep exact line geometry/confidence, plus a separate LF-joined page
        // projection so ordinary multiword queries can cross visual lines.
        // Quoted queries still match that representation's exact whitespace.
        let mut page = Spool::new(scratch)?;
        let mut offset = 0u64;
        let report = craft_content::ocr::recognize(runtime, image, sha256, bytes, scratch, source.clone(), &self.cancellation, |record| {
            let text = if offset == 0 { record.text.clone() } else { format!("\n{}", record.text) };
            let length = text.len() as u64;
            page.record(Record {
                location: json!({"source":source,"representation":"canonicalOcrPageText",
                "lineSeparator":"LF","machineRecognized":true}),
                offset,
                text,
                derivation: record.derivation.clone(),
            })?;
            offset = offset.checked_add(length).ok_or("OCR page text offset overflow")?;
            located(spool, record, coverage)
        });
        spool.append(&mut page)?;
        match report {
            Ok(report) => coverage.sources.push(json!({"location":source,"ocr":report})),
            Err(why) => coverage.gap(&source, failure(&why), why),
        }
        Ok(())
    }
}

fn extension(name: &str, mime: &str, bytes: &[u8]) -> String {
    if bytes.starts_with(b"%PDF-") {
        return "pdf".into();
    }
    let extension = name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).unwrap_or_default();
    if !extension.is_empty() && extension.len() <= 16 && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return extension;
    }
    match mime {
        "application/pdf" => "pdf",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "message/rfc822" => "eml",
        "text/html" => "html",
        _ => "",
    }
    .into()
}

#[cfg(windows)]
fn located(spool: &mut Spool, record: Record, coverage: &mut Coverage) -> Result<()> {
    let count = coverage.text_bytes.checked_add(record.text.len() as u64).ok_or("extracted text byte count overflow")?;
    if count > EXTRACTED_TEXT {
        return Err("resourceLimit: extracted native/OCR text exceeds the 256 MiB containing-attachment allowance".into());
    }
    spool.record(record)?;
    coverage.text_bytes = count;
    Ok(())
}
