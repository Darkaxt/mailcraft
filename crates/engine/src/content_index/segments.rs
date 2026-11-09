// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Verified UTF-8 streams become bounded overlapping source segments. Nothing
//! is committed until the source reaches its verified EOF and the spool flushes.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::search::text::word_edge;
use serde_json::Value;

use super::db::{OVERLAP_BYTES, SEGMENT_BYTES, Segment};

type Result<T> = std::result::Result<T, String>;
const MAX_LINE: usize = 2 * 1024 * 1024;

pub struct Spool {
    file: File,
    pub segments: u64,
    pub source_bytes: u64,
    active: Option<LocatedUnit>,
}

struct LocatedUnit {
    location: Value,
    derivation: String,
    buffer: String,
    start: u64,
    next: u64,
    starts_word: bool,
}

impl Spool {
    pub fn new(directory: &std::path::Path) -> Result<Self> {
        let file = tempfile::tempfile_in(directory).map_err(|why| why.to_string())?;
        Ok(Self { file, segments: 0, source_bytes: 0, active: None })
    }

    pub fn text(&mut self, source: impl Read, location: Value, derivation: &str, cancelled: &AtomicBool) -> Result<String> {
        self.finish_located()?;
        let mut output = BufWriter::new(&mut self.file);
        let mut buffer = String::new();
        let mut start = 0u64;
        let mut starts_word = true;
        let mut snippet = String::new();
        let mut snippets = 0usize;
        let mut count = 0u64;
        let mut bytes = 0u64;
        utf8(source, cancelled, |text| {
            bytes = bytes.checked_add(text.len() as u64).ok_or("text source byte count overflow")?;
            for ch in text.chars().take(8192usize.saturating_sub(snippets)) {
                snippet.push(ch);
                snippets += 1;
            }
            buffer.push_str(text);
            while buffer.len() > SEGMENT_BYTES + OVERLAP_BYTES {
                let mut end = SEGMENT_BYTES + OVERLAP_BYTES;
                while !buffer.is_char_boundary(end) {
                    end = end.saturating_sub(1);
                }
                let mut advance = end.saturating_sub(OVERLAP_BYTES);
                while !buffer.is_char_boundary(advance) {
                    advance = advance.saturating_sub(1);
                }
                let raw = buffer.get(..end).ok_or("invalid text segment boundary")?;
                let ends_word = buffer.get(end..).and_then(|rest| rest.chars().next()).is_none_or(|ch| !word_edge(ch));
                write_segment(
                    &mut output,
                    &Segment { location: location.clone(), offset: start, raw: raw.into(), starts_word, ends_word, derivation: derivation.into() },
                )?;
                count += 1;
                starts_word = buffer.get(..advance).and_then(|before| before.chars().next_back()).is_none_or(|ch| !word_edge(ch));
                buffer.drain(..advance);
                start = start.checked_add(advance as u64).ok_or("text source offset overflow")?;
            }
            Ok(())
        })?;
        if !buffer.is_empty() {
            write_segment(
                &mut output,
                &Segment { location, offset: start, raw: buffer, starts_word, ends_word: true, derivation: derivation.into() },
            )?;
            count += 1;
        }
        output.flush().map_err(|why| why.to_string())?;
        self.segments = self.segments.checked_add(count).ok_or("text segment count overflow")?;
        self.source_bytes = self.source_bytes.checked_add(bytes).ok_or("text source byte count overflow")?;
        Ok(snippet)
    }

    /// Native/OCR batches may split a paragraph or cell many times. Reassemble
    /// only the bounded overlap window, preserving one original source unit and
    /// its exact byte continuation instead of treating each reply as a new word.
    pub fn record(&mut self, record: craft_content::Record) -> Result<()> {
        if record.text.len() > craft_content::MAX_BATCH {
            return Err("the native text record exceeds its bounded source-fragment allowance".into());
        }
        let derivation = serde_json::to_string(&record.derivation).map_err(|why| why.to_string())?;
        if self.active.as_ref().is_some_and(|unit| unit.location != record.location || unit.derivation != derivation) {
            self.finish_located()?;
        }
        if self.active.is_none() {
            if record.offset != 0 {
                return Err("the native extractor began a source unit after its first byte".into());
            }
            self.active =
                Some(LocatedUnit { location: record.location.clone(), derivation, buffer: String::new(), start: 0, next: 0, starts_word: true });
        }
        let unit = self.active.as_mut().ok_or("the native text unit is unavailable")?;
        if record.offset != unit.next {
            return Err("the native extractor skipped or repeated an original text continuation".into());
        }
        unit.next = unit.next.checked_add(record.text.len() as u64).ok_or("native text source offset overflow")?;
        self.source_bytes = self.source_bytes.checked_add(record.text.len() as u64).ok_or("native text byte count overflow")?;
        unit.buffer.push_str(&record.text);
        while unit.buffer.len() > SEGMENT_BYTES + OVERLAP_BYTES {
            let mut end = SEGMENT_BYTES + OVERLAP_BYTES;
            while !unit.buffer.is_char_boundary(end) {
                end = end.saturating_sub(1)
            }
            let mut advance = end.saturating_sub(OVERLAP_BYTES);
            while !unit.buffer.is_char_boundary(advance) {
                advance = advance.saturating_sub(1)
            }
            let raw = unit.buffer.get(..end).ok_or("invalid native text segment boundary")?;
            let ends_word = unit.buffer.get(end..).and_then(|after| after.chars().next()).is_none_or(|ch| !word_edge(ch));
            write_segment(
                &mut self.file,
                &Segment {
                    location: unit.location.clone(),
                    offset: unit.start,
                    raw: raw.into(),
                    starts_word: unit.starts_word,
                    ends_word,
                    derivation: unit.derivation.clone(),
                },
            )?;
            self.segments += 1;
            unit.starts_word = unit.buffer.get(..advance).and_then(|before| before.chars().next_back()).is_none_or(|ch| !word_edge(ch));
            unit.start = unit.start.checked_add(advance as u64).ok_or("native text source offset overflow")?;
            unit.buffer.drain(..advance);
        }
        Ok(())
    }

    fn finish_located(&mut self) -> Result<()> {
        if let Some(unit) = self.active.take()
            && !unit.buffer.is_empty()
        {
            write_segment(
                &mut self.file,
                &Segment {
                    location: unit.location,
                    offset: unit.start,
                    raw: unit.buffer,
                    starts_word: unit.starts_word,
                    ends_word: true,
                    derivation: unit.derivation,
                },
            )?;
            self.segments += 1;
        }
        Ok(())
    }

    pub fn records(&mut self) -> Result<Records<'_>> {
        self.finish_located()?;
        self.file.sync_all().map_err(|why| why.to_string())?;
        self.file.seek(SeekFrom::Start(0)).map_err(|why| why.to_string())?;
        Ok(Records { reader: BufReader::new(&mut self.file), failed: false })
    }

    pub fn append(&mut self, other: &mut Spool) -> Result<()> {
        self.finish_located()?;
        self.file.seek(SeekFrom::End(0)).map_err(|why| why.to_string())?;
        for record in other.records()? {
            write_segment(&mut self.file, &record?)?;
            self.segments = self.segments.checked_add(1).ok_or("extraction segment count overflow")?;
        }
        self.source_bytes = self.source_bytes.checked_add(other.source_bytes).ok_or("indexed representation byte count overflow")?;
        Ok(())
    }
}

fn write_segment(output: &mut impl Write, segment: &Segment) -> Result<()> {
    serde_json::to_writer(&mut *output, segment).map_err(|why| why.to_string())?;
    output.write_all(b"\n").map_err(|why| why.to_string())
}

pub struct Records<'a> {
    reader: BufReader<&'a mut File>,
    failed: bool,
}

impl Iterator for Records<'_> {
    type Item = Result<Segment>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        let mut line = Vec::new();
        let result = self.reader.by_ref().take((MAX_LINE + 1) as u64).read_until(b'\n', &mut line);
        match result {
            Ok(0) => None,
            Ok(_) if line.len() > MAX_LINE || line.last() != Some(&b'\n') => {
                self.failed = true;
                Some(Err("the extraction spool has an oversized or incomplete record".into()))
            }
            Ok(_) => Some(serde_json::from_slice(&line).map_err(|why| why.to_string())),
            Err(why) => {
                self.failed = true;
                Some(Err(why.to_string()))
            }
        }
    }
}

/// Invalid UTF-8 is an explicit failed source, never silent replacement text.
pub fn utf8(mut source: impl Read, cancelled: &AtomicBool, mut accept: impl FnMut(&str) -> Result<()>) -> Result<()> {
    let mut chunk = [0u8; 32 * 1024];
    let mut pending = Vec::with_capacity(chunk.len() + 4);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("indexing cancelled".into());
        }
        let read = source.read(&mut chunk).map_err(|why| why.to_string())?;
        if read == 0 {
            if !pending.is_empty() {
                return Err("the text source ends in an incomplete UTF-8 character".into());
            }
            return Ok(());
        }
        pending.extend_from_slice(chunk.get(..read).ok_or("invalid text stream length")?);
        match std::str::from_utf8(&pending) {
            Ok(text) => {
                accept(text)?;
                pending.clear();
            }
            Err(why) => {
                if why.error_len().is_some() {
                    return Err("the retained text source is not valid UTF-8".into());
                }
                let valid = why.valid_up_to();
                accept(std::str::from_utf8(pending.get(..valid).ok_or("invalid UTF-8 boundary")?).map_err(|why| why.to_string())?)?;
                pending.drain(..valid);
            }
        }
    }
}
