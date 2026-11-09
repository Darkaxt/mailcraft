// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Private finite core-worker protocol. Opaque request ids are echoed unchanged;
//! an uncertain request is not automatically repeated. No command launches a GUI.

use std::io::{BufRead, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Batch, Cursor, MAX_BATCH, VERSION};

pub const MAX_REQUEST: usize = 8192;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub version: u32,
    pub id: u64,
    #[serde(flatten)]
    pub operation: Operation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum Operation {
    Open { input: PathBuf, sha256: String, extension: String, scratch: PathBuf },
    Info,
    Extract { cursor: Cursor, limit: usize },
    Render { part: u32, page: u32, width: u32 },
    Close,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Rendered {
    pub path: PathBuf,
    pub kind: String,
    pub sha256: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Reply {
    pub version: u32,
    pub id: u64,
    pub result: Option<ResultValue>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum ResultValue {
    Opened(Value),
    Info(Value),
    Text(Batch),
    Rendered(Rendered),
    Closed,
}

pub fn read<T: serde::de::DeserializeOwned>(input: &mut impl BufRead, max: usize) -> Result<Option<T>, String> {
    let mut line = Vec::new();
    let count = input.by_ref().take(max.saturating_add(1) as u64).read_until(b'\n', &mut line).map_err(|why| why.to_string())?;
    if count == 0 {
        return Ok(None);
    }
    if count > max || line.last() != Some(&b'\n') {
        return Err("the Craft content protocol frame is oversized or incomplete".into());
    }
    serde_json::from_slice(&line).map(Some).map_err(|why| why.to_string())
}

pub fn write<T: Serialize>(output: &mut impl Write, value: &T, max: usize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|why| why.to_string())?;
    if bytes.len().saturating_add(1) > max {
        return Err("the Craft content protocol reply exceeds its serialized byte limit".into());
    }
    output.write_all(&bytes).and_then(|_| output.write_all(b"\n")).and_then(|_| output.flush()).map_err(|why| why.to_string())
}

impl Reply {
    pub fn new(id: u64, result: Result<ResultValue, String>) -> Self {
        match result {
            Ok(result) => Self { version: VERSION, id, result: Some(result), error: None },
            Err(error) => Self { version: VERSION, id, result: None, error: Some(error) },
        }
    }

    pub fn check(self, id: u64) -> Result<ResultValue, String> {
        if self.version != VERSION || self.id != id || self.error.is_some() == self.result.is_some() {
            return Err("the Craft worker response has another identity, version or outcome".into());
        }
        self.result.ok_or_else(|| self.error.unwrap_or_else(|| "the Craft worker returned no result".into()))
    }
}

pub const MAX_REPLY: usize = MAX_BATCH + 8192;
