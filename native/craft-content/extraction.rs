// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Adapter for the existing core methods. The old Pathweft render ABI remains
//! unchanged in its source; this worker returns the shared located-text version.

use serde_json::{Value, json};

pub fn page<T: FnOnce() -> String>(units: impl IntoIterator<Item = (Value,T)>,
    unit: u64, offset: usize, limit: usize, coverage: &str) -> Result<Value,String> {
    let batch = craft_content::paging::page(units,craft_content::Cursor { unit,offset:offset as u64 },limit,coverage.into(),Vec::new())?;
    let mut value = serde_json::to_value(batch).map_err(|why| why.to_string())?;
    // The PDF adapter uses this optional legacy hint to label image-only pages.
    // It does not change the shared records, cursor, coverage or rendering API.
    value["status"] = json!("text");
    Ok(value)
}
