// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded continuation of native text units. Empty units and the end of one
//! batch never imply that later units are empty. A cursor is always explicit.

use serde_json::Value;

use crate::{Batch, Cursor, Derivation, Gap, MAX_BATCH, Record, VERSION};

pub fn page<T: FnOnce() -> String>(
    units: impl IntoIterator<Item = (Value, T)>,
    cursor: Cursor,
    limit: usize,
    coverage: String,
    gaps: Vec<Gap>,
) -> Result<Batch, String> {
    if !(1024..=MAX_BATCH).contains(&limit) {
        return Err("invalid Craft content reply byte limit".into());
    }
    let mut batch = Batch { version: VERSION, records: Vec::new(), next: None, coverage, gaps };
    let start_unit = usize::try_from(cursor.unit).map_err(|why| why.to_string())?;
    let mut saw_cursor = false;
    for (index, (location, read_text)) in units.into_iter().enumerate().skip(start_unit) {
        saw_cursor = true;
        let text = read_text();
        let start = if index == start_unit { usize::try_from(cursor.offset).map_err(|why| why.to_string())? } else { 0 };
        if start > text.len() || !text.is_char_boundary(start) {
            return Err("invalid Craft content text continuation offset".into());
        }
        if text.is_empty() || start == text.len() {
            continue;
        }
        // Reserve the largest possible continuation before sizing this record.
        batch.next = Some(Cursor { unit: index as u64, offset: text.len() as u64 });
        let occupied = serde_json::to_vec(&batch).map_err(|why| why.to_string())?.len();
        let record_overhead = serde_json::to_vec(&location).map_err(|why| why.to_string())?.len().saturating_add(160);
        let room = limit.saturating_sub(occupied.saturating_add(record_overhead)) / 6;
        let remaining = text.get(start..).ok_or("invalid Craft text boundary")?;
        let end = remaining
            .char_indices()
            .take_while(|(at, ch)| at.saturating_add(ch.len_utf8()) <= room)
            .last()
            .map_or(start, |(at, ch)| start.saturating_add(at).saturating_add(ch.len_utf8()));
        if end == start {
            if batch.records.is_empty() {
                return Err("Craft location or coverage exceeds the reply byte limit".into());
            }
            batch.next = Some(Cursor { unit: index as u64, offset: start as u64 });
            return checked(batch, limit);
        }
        batch.records.push(Record {
            location,
            offset: start as u64,
            text: text.get(start..end).ok_or("invalid Craft text boundary")?.into(),
            derivation: Derivation::Native,
        });
        if end < text.len() {
            batch.next = Some(Cursor { unit: index as u64, offset: end as u64 });
            return checked(batch, limit);
        }
    }
    if !saw_cursor && (cursor.unit != 0 || cursor.offset != 0) {
        return Err("Craft content continuation is outside the document".into());
    }
    batch.next = None;
    checked(batch, limit)
}

fn checked(batch: Batch, limit: usize) -> Result<Batch, String> {
    batch.check()?;
    if serde_json::to_vec(&batch).map_err(|why| why.to_string())?.len() > limit {
        return Err("Craft content reply exceeds the requested byte limit".into());
    }
    Ok(batch)
}
