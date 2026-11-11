// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local Tesseract recognition over verified rendered/image bytes. Models and
//! their reported languages are selected package payloads; nothing is fetched.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    Derivation, Record,
    process::{Cancellation, ManagedProcess},
    runtime::{Runtime, verify},
};

type Result<T> = std::result::Result<T, String>;
const MAX_TSV_LINE: usize = 64 * 1024;
const MAX_TEXT_LINE: usize = 24 * 1024;
/// Generous wall clock for recognizing one image; a hung OCR process is ended
/// and reported as a resourceLimit gap instead of blocking the content writer.
pub const OCR_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);
const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub engine: String,
    pub version: String,
    pub languages: Vec<String>,
    pub lines: u64,
    pub words: u64,
    pub warnings: String,
}

struct Line {
    id: Vec<u32>,
    text: String,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    min_confidence: f64,
}

fn number(fields: &[&str], index: usize) -> Result<u32> {
    fields.get(index).ok_or("the OCR output has an incomplete position")?.parse().map_err(|_| "the OCR output has an invalid position".into())
}

fn emit(line: Line, location: &Value, report: &Report, accept: &mut impl FnMut(Record) -> Result<()>) -> Result<()> {
    accept(Record {
        location: json!({"source":location,"ocrLine":line.id,
        "rect":{"left":line.left,"top":line.top,"width":line.right.saturating_sub(line.left),"height":line.bottom.saturating_sub(line.top)},
        "space":"rendered image pixels, top-left origin, y down","minimumWordConfidence":line.min_confidence}),
        offset: 0,
        text: line.text,
        derivation: Derivation::Ocr { engine: "Tesseract".into(), version: report.version.clone(), languages: report.languages.clone() },
    })
}

fn read_line(input: &mut impl BufRead, max: usize) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let count = input.by_ref().take(max.saturating_add(1) as u64).read_until(b'\n', &mut bytes).map_err(|why| why.to_string())?;
    if count == 0 {
        return Ok(None);
    }
    if count > max || bytes.last() != Some(&b'\n') {
        return Err("the OCR output has an oversized or incomplete TSV record".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "the OCR output is not valid UTF-8".to_owned())?;
    Ok(Some(text.trim_end_matches(['\r', '\n']).into()))
}

pub fn recognize(
    runtime: &Runtime,
    image: &Path,
    sha256: &str,
    bytes: u64,
    scratch: &Path,
    location: Value,
    cancellation: &Cancellation,
    mut accept: impl FnMut(Record) -> Result<()>,
) -> Result<Report> {
    if bytes > crate::MAX_DECODED {
        return Err("the OCR image exceeds its decoded input allowance".into());
    }
    verify(image, sha256, bytes)?;
    // English-first Tesseract recognition can give high-confidence Latin
    // lookalikes for Greek and discard Spanish accents. Keep the selected
    // specialist models' order and use English as their fallback. The actual
    // invocation order is retained with every located record and report.
    let mut languages = runtime.manifest.ocr.languages.clone();
    languages.sort_by_key(|language| language == "eng");
    let executable = runtime.ocr_executable()?;
    let mut stderr = tempfile::tempfile_in(scratch).map_err(|why| why.to_string())?;
    let mut command = Command::new(executable);
    command
        .args(["stdin", "stdout", "--oem", "1", "--psm", "3", "--tessdata-dir"])
        .arg(runtime.tessdata()?)
        .arg("-l")
        .arg(languages.join("+"))
        .args(["-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr.try_clone().map_err(|why| why.to_string())?)
        .current_dir(scratch)
        .env("TEMP", scratch)
        .env("TMP", scratch)
        .env("OMP_THREAD_LIMIT", "1")
        .env("OMP_NUM_THREADS", "1");
    let mut process = ManagedProcess::spawn(command, cancellation)?;
    process.deadline(Some(OCR_DEADLINE));
    let input = process.stdin().ok_or("the OCR worker has no owned image input")?;
    let output = process.stdout().ok_or("the OCR worker has no owned text output")?;
    let mut image = File::open(image).map_err(|why| why.to_string())?;
    // Feed concurrently with bounded output consumption: a decoder that stops
    // early cannot deadlock image writing against a full text-output pipe.
    let writer = std::thread::Builder::new()
        .name("craft-ocr-image".into())
        .spawn(move || {
            let mut input = input;
            std::io::copy(&mut image, &mut input).map_err(|why| why.to_string())
        })
        .map_err(|why| why.to_string())?;
    let mut output = BufReader::new(output);
    let mut report =
        Report { engine: "Tesseract".into(), version: runtime.manifest.ocr.version.clone(), languages, lines: 0, words: 0, warnings: String::new() };
    let parsed = (|| -> Result<()> {
        if read_line(&mut output, MAX_TSV_LINE)?.as_deref() != Some(HEADER) {
            return Err("the original OCR process did not return its declared TSV format".into());
        }
        let mut line = None::<Line>;
        while let Some(record) = read_line(&mut output, MAX_TSV_LINE)? {
            if cancellation.cancelled() {
                return Err("OCR cancelled".into());
            }
            let fields: Vec<&str> = record.splitn(12, '\t').collect();
            if fields.len() != 12 {
                return Err("the OCR output has an incomplete TSV record".into());
            }
            if number(&fields, 0)? != 5 {
                continue;
            }
            let text = fields.get(11).copied().ok_or("the OCR word is missing")?;
            if text.is_empty() {
                continue;
            }
            let id = (1..5).map(|index| number(&fields, index)).collect::<Result<Vec<_>>>()?;
            if line.as_ref().is_some_and(|line| line.id != id)
                && let Some(previous) = line.take()
            {
                emit(previous, &location, &report, &mut accept)?;
                report.lines += 1;
            }
            let (left, top, width, height) = (number(&fields, 6)?, number(&fields, 7)?, number(&fields, 8)?, number(&fields, 9)?);
            let (right, bottom) =
                (left.checked_add(width).ok_or("OCR word geometry overflow")?, top.checked_add(height).ok_or("OCR word geometry overflow")?);
            let confidence: f64 = fields.get(10).ok_or("the OCR confidence is missing")?.parse().map_err(|_| "invalid OCR word confidence")?;
            if !confidence.is_finite() || !(0.0..=100.0).contains(&confidence) {
                return Err("the OCR worker returned an invalid word confidence".into());
            }
            let line = line.get_or_insert_with(|| Line { id, text: String::new(), left, top, right, bottom, min_confidence: confidence });
            if line.text.len().saturating_add(text.len()).saturating_add(1) > MAX_TEXT_LINE {
                return Err("an OCR text line exceeds its explicit located-record allowance".into());
            }
            if !line.text.is_empty() {
                line.text.push(' ')
            }
            line.text.push_str(text);
            line.left = line.left.min(left);
            line.top = line.top.min(top);
            line.right = line.right.max(right);
            line.bottom = line.bottom.max(bottom);
            line.min_confidence = line.min_confidence.min(confidence);
            report.words += 1;
        }
        if let Some(line) = line {
            emit(line, &location, &report, &mut accept)?;
            report.lines += 1;
        }
        Ok(())
    })();
    if parsed.is_err() {
        process.abort()
    }
    let written = writer.join().map_err(|_| "the OCR image writer panicked".to_owned())?;
    if written.is_err() {
        process.abort()
    }
    let status = process.wait()?;
    if process.expired() {
        return Err(format!(
            "resourceLimit: OCR did not finish within its {}-second deadline and was ended; original OCR PID {}",
            OCR_DEADLINE.as_secs(),
            process.id()
        ));
    }
    stderr.seek(SeekFrom::Start(0)).map_err(|why| why.to_string())?;
    stderr.by_ref().take(8193).read_to_string(&mut report.warnings).map_err(|why| why.to_string())?;
    if report.warnings.len() > 8192 {
        return Err("the OCR worker emitted diagnostics beyond its explicit reporting allowance".into());
    }
    let details = format!("original OCR PID {}, exit {status}; {}", process.id(), report.warnings);
    parsed.map_err(|why| format!("{why}; {details}"))?;
    if written.map_err(|why| format!("{why}; {details}"))? != bytes {
        return Err(format!("the OCR worker did not receive the complete verified image; {details}"));
    }
    if !status.success() {
        return Err(format!("the original OCR process failed; its known output remains incomplete; {details}"));
    }
    Ok(report)
}
