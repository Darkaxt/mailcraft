// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Located text shared by Craft core workers and their consumers. No mailbox,
//! indexing engine, GUI, network request or application settings are involved.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(feature = "html")]
pub mod html;
#[cfg(all(windows, feature = "runtime"))]
pub mod ocr;
pub mod paging;
#[cfg(all(windows, feature = "runtime"))]
pub mod process;
pub mod protocol;
#[cfg(all(windows, feature = "runtime"))]
pub mod runtime;

pub const VERSION: u32 = 1;
pub const MAX_BATCH: usize = 48 * 1024;
pub const MAX_INPUT: u64 = 64 * 1024 * 1024;
pub const MAX_DECODED: u64 = 256 * 1024 * 1024;
pub const MAX_PIXELS: u64 = 64_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub unit: u64,
    pub offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Derivation {
    Native,
    Ocr { engine: String, version: String, languages: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// Structured native paragraph, sheet/cell, slide/object or page/rectangle.
    pub location: Value,
    /// UTF-8 bytes within this text unit, never a character or message offset.
    pub offset: u64,
    pub text: String,
    pub derivation: Derivation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub location: Value,
    /// Stable code: missing, encrypted, unsupported, malformed or resourceLimit.
    pub code: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub version: u32,
    pub records: Vec<Record>,
    pub next: Option<Cursor>,
    pub coverage: String,
    pub gaps: Vec<Gap>,
}

impl Batch {
    pub fn check(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err("unsupported Craft content protocol version".into());
        }
        let bytes = serde_json::to_vec(self).map_err(|why| why.to_string())?;
        if bytes.len() > MAX_BATCH {
            return Err("Craft content batch exceeds its serialized byte limit".into());
        }
        if self.records.iter().any(|record| record.offset.checked_add(record.text.len() as u64).is_none()) {
            return Err("Craft text source offset overflow".into());
        }
        Ok(())
    }
}
