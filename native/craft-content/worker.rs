// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Staged into each pinned donor workspace as one finite console worker. No
//! donor GUI/CLI/MCP or application-owned credentials/settings are linked.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use craft_content::{Batch, MAX_INPUT, MAX_PIXELS, VERSION};
use craft_content::protocol::{self, Operation, Rendered, Reply, Request, ResultValue};
use image::ImageEncoder;
use serde_json::Value;

mod engine;
mod extraction;

pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// 0: premultiplied RGBA; 1: complete in-memory GridCraft print PDF.
    pub format: u32,
    pub pixels: Vec<u8>,
}

struct Document {
    engine: engine::Engine,
    scratch: PathBuf,
    rendered: Option<tempfile::NamedTempFile>,
}

fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn open(input: &std::path::Path, expected: &str, extension: &str, scratch: &std::path::Path) -> Result<Document, String> {
    if extension.is_empty() || extension.len() > 16 || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err("invalid Craft input extension".into());
    }
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the Craft input has no independently supplied SHA-256".into());
    }
    if std::fs::symlink_metadata(scratch).map_err(|why| why.to_string())?.file_type().is_symlink() {
        return Err("the private worker scratch must not be a link".into());
    }
    let scratch = scratch.canonicalize().map_err(|why| why.to_string())?;
    if !scratch.is_dir() || input.parent().and_then(|path| path.canonicalize().ok()).as_ref() != Some(&scratch) {
        return Err("the Craft worker input must be in its exact private scratch directory".into());
    }
    let metadata = std::fs::symlink_metadata(input).map_err(|why| why.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_INPUT {
        return Err("the native input is not a regular file within the 64 MiB import allowance".into());
    }
    let mut file = File::open(input).map_err(|why| why.to_string())?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(MAX_INPUT + 1).read_to_end(&mut bytes).map_err(|why| why.to_string())?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_INPUT {
        return Err("the Craft input changed or exceeded its import allowance".into());
    }
    if hexadecimal(ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref()) != expected.to_ascii_lowercase() {
        return Err("the Craft worker input does not match its supplied SHA-256".into());
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        // Bound compressed document containers before the importer allocates
        // their model. These are declared native-import limits, not a claim
        // that input length is an OS committed-memory quota.
        let mut archive=zip::ZipArchive::new(std::io::Cursor::new(&bytes)).map_err(|why|format!("malformed: {why}"))?;
        if archive.len()>20_000 {return Err("resourceLimit: native document container exceeds 20,000 entries".into());}
        let mut decoded=0u64;
        for index in 0..archive.len() {
            let file=archive.by_index_raw(index).map_err(|why|format!("malformed: {why}"))?;
            decoded=decoded.checked_add(file.size()).ok_or("resourceLimit: native container decoded size overflow")?;
            if decoded>128*1024*1024 {return Err("resourceLimit: native document container exceeds 128 MiB declared decoded content".into());}
        }
    }
    let engine = engine::Engine::open(&bytes, extension)?;
    Ok(Document { engine,scratch,rendered:None })
}

impl Document {
    fn render(&mut self, part: u32, page: u32, width: u32) -> Result<Rendered, String> {
        if width == 0 || width > 4096 {
            return Err("the Craft render width must be within 1..4096 pixels".into());
        }
        let mut raster = self.engine.render(part, page, width)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.scratch).map_err(|why| why.to_string())?;
        let kind = if raster.format == 1 {
            if raster.pixels.len() as u64 > craft_content::MAX_DECODED {
                return Err("the GridCraft print PDF exceeds the decoded allowance".into());
            }
            file.write_all(&raster.pixels).map_err(|why| why.to_string())?;
            "pdf"
        } else if raster.format == 0 {
            let pixels = u64::from(raster.width).checked_mul(u64::from(raster.height)).ok_or("Craft raster dimensions overflow")?;
            if pixels == 0 || pixels > MAX_PIXELS || pixels.checked_mul(4) != Some(raster.pixels.len() as u64) {
                return Err("the Craft raster is incomplete or exceeds its decoded pixel allowance".into());
            }
            // Composite premultiplied RGBA on white for OCR. No colour transform
            // is guessed from the alpha channel and no external image is read.
            for pixel in raster.pixels.chunks_exact_mut(4) {
                let alpha = pixel.get(3).copied().ok_or("invalid Craft raster pixel")?;
                for channel in pixel.iter_mut().take(3) {
                    *channel = channel.saturating_add(255u8.saturating_sub(alpha));
                }
                if let Some(alpha) = pixel.get_mut(3) {
                    *alpha = 255;
                }
            }
            image::codecs::png::PngEncoder::new(file.as_file_mut()).write_image(&raster.pixels,
                raster.width,raster.height,image::ExtendedColorType::Rgba8).map_err(|why| why.to_string())?;
            "png"
        } else {
            return Err("the Craft core returned an unsupported raster representation".into());
        };
        file.flush().and_then(|_| file.as_file().sync_all()).map_err(|why| why.to_string())?;
        let bytes = file.as_file().metadata().map_err(|why| why.to_string())?.len();
        file.as_file_mut().seek(SeekFrom::Start(0)).map_err(|why| why.to_string())?;
        let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = [0u8; 32 * 1024];
        loop {
            let count = file.read(&mut buffer).map_err(|why| why.to_string())?;
            if count == 0 { break }
            hash.update(buffer.get(..count).ok_or("invalid rendered content read")?);
        }
        let result = Rendered { path:file.path().into(),kind:kind.into(),sha256:hexadecimal(hash.finish().as_ref()),
            bytes,width:raster.width,height:raster.height };
        self.rendered = Some(file);
        Ok(result)
    }
}

fn perform(document: &mut Option<Document>, request: Operation) -> Result<ResultValue, String> {
    match request {
        Operation::Open { input,sha256,extension,scratch } => {
            if document.is_some() { return Err("close the current Craft document before opening another".into()) }
            let opened = open(&input,&sha256,&extension,&scratch)?;
            let info = opened.engine.info();
            *document = Some(opened);
            Ok(ResultValue::Opened(info))
        }
        Operation::Info => Ok(ResultValue::Info(document.as_ref().ok_or("no Craft document is open")?.engine.info())),
        Operation::Extract { cursor,limit } => {
            let document = document.as_ref().ok_or("no Craft document is open")?;
            let value: Value = document.engine.extract(cursor.unit,usize::try_from(cursor.offset).map_err(|why| why.to_string())?,limit)?;
            let batch: Batch = serde_json::from_value(value).map_err(|why| why.to_string())?;
            batch.check()?;
            Ok(ResultValue::Text(batch))
        }
        Operation::Render { part,page,width } => Ok(ResultValue::Rendered(document.as_mut().ok_or("no Craft document is open")?.render(part,page,width)?)),
        Operation::Close => {
            *document = None;
            Ok(ResultValue::Closed)
        }
    }
}

fn run() -> Result<(), String> {
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = std::io::stdout().lock();
    let mut document = None;
    while let Some(request) = protocol::read::<Request>(&mut input,protocol::MAX_REQUEST)? {
        let closing = matches!(&request.operation,Operation::Close);
        let result = if request.version != VERSION {
            Err("unsupported Craft content protocol version".into())
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| perform(&mut document,request.operation)))
                .unwrap_or_else(|_| Err("the native Craft core could not complete this document operation".into()))
        };
        protocol::write(&mut output,&Reply::new(request.id,result),protocol::MAX_REPLY)?;
        if closing { return Ok(()) }
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("Craft content worker: {why}");
            std::process::ExitCode::FAILURE
        }
    }
}
