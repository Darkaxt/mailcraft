// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded leaf inspection of attached RFC 5322 messages. Unlike display-body
//! selection this visits every alternative, with explicit exclusions. It does
//! not import mail, run message content, or alter the established reader.

use crate::{header, parse::split_multipart, transfer};

pub const MAX_PARTS: usize = 1000;
pub const MAX_DEPTH: usize = 20;
pub const MAX_INPUT: usize = 64 * 1024 * 1024;

pub struct Leaf<'a> {
    pub path: Vec<usize>,
    pub mime: String,
    pub charset: String,
    pub name: String,
    pub header: &'a [u8],
    pub decoded: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    pub path: Vec<usize>,
    pub code: &'static str,
    pub detail: &'static str,
}

#[derive(Default)]
pub struct Report {
    pub visited: usize,
    pub leaves: usize,
    pub decoded_bytes: u64,
    pub gaps: Vec<Gap>,
}

/// `max_decoded` is a shared remaining byte allowance supplied by the caller for
/// the containing attachment, including any recursively attached messages.
pub fn visit(bytes: &[u8], max_decoded: u64, mut accept: impl FnMut(Leaf<'_>) -> Result<(), String>) -> Result<Report, String> {
    let mut report = Report::default();
    if bytes.len() > MAX_INPUT {
        report.gaps.push(Gap { path: Vec::new(), code: "resourceLimit", detail: "nested MIME input exceeds 64 MiB" });
        return Ok(report);
    }
    let mut stack = vec![(bytes, Vec::<usize>::new())];
    while let Some((part, path)) = stack.pop() {
        if report.visited >= MAX_PARTS {
            report.gaps.push(Gap { path, code: "resourceLimit", detail: "nested MIME input exceeds 1000 visited parts" });
            break;
        }
        report.visited += 1;
        let (headers, body) = header::split(part, path.is_empty());
        let media = headers.get("content-type").map(header::parse_params).unwrap_or_default();
        let mime = if media.value.is_empty() { "text/plain" } else { &media.value };
        if mime.starts_with("multipart/") {
            if path.len() >= MAX_DEPTH {
                report.gaps.push(Gap { path, code: "resourceLimit", detail: "nested multipart depth exceeds 20" });
                continue;
            }
            let Some(boundary) = media.get("boundary").filter(|boundary| !boundary.is_empty()) else {
                report.gaps.push(Gap { path, code: "malformed", detail: "multipart source has no usable boundary" });
                continue;
            };
            let remaining = MAX_PARTS.saturating_sub(report.visited);
            let pieces = split_multipart(body, boundary, remaining.saturating_add(1));
            if pieces.is_empty() {
                report.gaps.push(Gap { path: path.clone(), code: "malformed", detail: "no declared MIME boundary was found" });
            }
            if pieces.len() > remaining {
                report.gaps.push(Gap { path: path.clone(), code: "resourceLimit", detail: "additional MIME parts exceed the 1000-part allowance" });
            }
            for (index, piece) in pieces.into_iter().take(remaining).enumerate().rev() {
                let mut child = path.clone();
                child.push(index);
                stack.push((piece, child));
            }
            continue;
        }
        let encoding = headers.get("content-transfer-encoding").unwrap_or_default().trim().to_ascii_lowercase();
        let decoded = match decode(body, &encoding) {
            Ok(decoded) => decoded,
            Err(detail) => {
                report.gaps.push(Gap { path, code: "malformed", detail });
                continue;
            }
        };
        let next_bytes = report.decoded_bytes.checked_add(decoded.len() as u64).ok_or("nested MIME decoded byte count overflow")?;
        if next_bytes > max_decoded {
            report.gaps.push(Gap { path, code: "resourceLimit", detail: "nested MIME decoded bytes exceed the containing attachment allowance" });
            continue;
        }
        report.decoded_bytes = next_bytes;
        let disposition = headers.get("content-disposition").map(header::parse_params).unwrap_or_default();
        let name = disposition.text("filename").or_else(|| media.text("name")).unwrap_or_default();
        let head = part.get(..part.len().saturating_sub(body.len())).ok_or("invalid MIME header boundary")?;
        accept(Leaf { path, mime: mime.into(), charset: media.get("charset").unwrap_or_default().into(), name, header: head, decoded })?;
        report.leaves += 1;
    }
    Ok(report)
}

fn decode(body: &[u8], encoding: &str) -> Result<Vec<u8>, &'static str> {
    match encoding {
        "" | "7bit" | "8bit" | "binary" => Ok(body.into()),
        "base64" => {
            let encoded: Vec<u8> = body.iter().copied().filter(|byte| !byte.is_ascii_whitespace()).collect();
            let config = if encoded.contains(&b'=') { mailcraft_charset::base64::STANDARD } else { mailcraft_charset::base64::STANDARD_NO_PAD };
            config.decode(&encoded).map_err(|_| "nested MIME source has invalid base64 rather than a complete decoded payload")
        }
        "quoted-printable" => {
            let mut at = 0usize;
            while let Some(byte) = body.get(at) {
                if *byte != b'=' {
                    at += 1;
                    continue;
                }
                let mut next = at.saturating_add(1);
                while matches!(body.get(next), Some(b' ' | b'\t')) {
                    next += 1
                }
                if matches!(body.get(next), Some(b'\r' | b'\n')) {
                    at = next.saturating_add(1);
                } else if body.get(at.saturating_add(1)).is_some_and(u8::is_ascii_hexdigit)
                    && body.get(at.saturating_add(2)).is_some_and(u8::is_ascii_hexdigit)
                {
                    at = at.saturating_add(3);
                } else {
                    return Err("nested MIME source has an invalid quoted-printable escape");
                }
            }
            Ok(transfer::qp_decode(body))
        }
        _ => Err("nested MIME source declares an unsupported transfer encoding"),
    }
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
