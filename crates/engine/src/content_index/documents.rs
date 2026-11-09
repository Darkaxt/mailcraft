// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One current document, content-addressed extraction parts and durable preview.
//! Missing/failed sources never become a successful negative search result.

use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use craft_content::{Derivation, Record};
use mailcraft_model::words;
use serde_json::{Value, json};

use super::{
    db::{Database, Link},
    input::{Input, hex, reference},
    segments::{self, Spool},
};
use crate::Part;

type Result<T> = std::result::Result<T, String>;

pub struct Completion {
    pub id: u64,
    pub fingerprint: String,
    pub needs_attachments: bool,
}

#[derive(Clone, Copy)]
pub enum Phase {
    Body,
    Attachments,
}

fn cache_key(kind: &str, bytes: &[u8]) -> String {
    format!("{}:{kind}:{}", super::input::EXTRACTOR_VERSION, hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref()))
}

fn stored(
    database: &mut Database,
    input: &Input,
    key: &str,
    fields: u8,
    source: Value,
    scratch: &Path,
    cancelled: &AtomicBool,
    produce: impl FnOnce(&mut Spool) -> Result<(bool, Value)>,
) -> Result<bool> {
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err("indexing cancelled".into());
    }
    let complete = if let Some((_, complete, _)) = database.part_ready(key)? {
        complete
    } else {
        let mut spool = Spool::new(scratch)?;
        let (complete, coverage) = match produce(&mut spool) {
            Ok(value) => value,
            Err(why) => {
                // EOF/hash/UTF-8 failures must not publish an earlier segment
                // from unverified bytes. The failed source has coverage only.
                spool = Spool::new(scratch)?;
                let code = if why.starts_with("resourceLimit:") { "resourceLimit" } else { "failed" };
                (false, json!({"complete":false,"gaps":[{"code":code,"detail":why}]}))
            }
        };
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err("indexing cancelled".into());
        }
        let codes = coverage
            .get("gaps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|gap| gap.get("code").and_then(Value::as_str))
            .collect::<Vec<_>>();
        let state = if complete {
            "indexed"
        } else if codes.contains(&"failed") || codes.contains(&"malformed") {
            "failed"
        } else if codes.contains(&"missing") {
            "missing"
        } else if codes.contains(&"unsupported") || codes.contains(&"resourceLimit") {
            "unsupported"
        } else {
            "partial"
        };
        database.part(key, state, complete, &coverage, spool.records()?)?;
        complete
    };
    database.link(&[Link { document: input.message.id, key: key.into(), fields, source, available: true }])?;
    Ok(complete)
}

fn headers(database: &mut Database, input: &Input, scratch: &Path, cancelled: &AtomicBool) -> Result<u8> {
    let message = &input.message;
    let groups = [
        (words::SUBJECT, "subject", vec![message.subject.clone()]),
        (words::FROM, "from", vec![message.from.name.clone(), message.from.email.clone(), message.from.full()]),
        (words::TO, "to", message.to.iter().flat_map(|address| [address.name.clone(), address.email.clone(), address.full()]).collect()),
        (words::CC, "cc", message.cc.iter().flat_map(|address| [address.name.clone(), address.email.clone(), address.full()]).collect()),
        (
            words::ATTACHMENT,
            "attachmentMetadata",
            message.attachments.iter().flat_map(|file| [file.name.clone(), file.mime.clone(), file.content_id.clone()]).collect(),
        ),
    ];
    let mut complete = 0;
    for (fields, field, texts) in groups {
        let encoded = serde_json::to_vec(&texts).map_err(|why| why.to_string())?;
        let key = cache_key(field, &encoded);
        if stored(database, input, &key, fields, json!({"field":field}), scratch, cancelled, |spool| {
            for (index, text) in texts.iter().enumerate() {
                spool.text(Cursor::new(text.as_bytes()), json!({"field":field,"entry":index}), "native", cancelled)?;
            }
            Ok((true, json!({"complete":true,"scope":"retained message metadata"})))
        })? {
            complete |= fields;
        }
    }
    // Reconstructed field labels are provenance, not words supplied by the
    // sender. Inserting "Subject:" would make a bare "su" match every message.
    // Preserve actual retained header names as their own source text instead.
    let mut text = vec![
        ("Subject".to_owned(), message.subject.clone()),
        ("From".into(), message.from.full()),
        ("To".into(), mailcraft_model::format_list(&message.to)),
        ("Cc".into(), mailcraft_model::format_list(&message.cc)),
        ("Bcc".into(), mailcraft_model::format_list(&message.bcc)),
        ("Reply-To".into(), mailcraft_model::format_list(&message.reply_to)),
        ("Message-ID".into(), message.message_id.clone()),
        ("In-Reply-To".into(), message.in_reply_to.clone()),
        ("References".into(), message.references.join(" ")),
        ("Date".into(), mailcraft_time::format_iso_utc(message.date)),
    ];
    let modeled = text.len();
    text.extend(message.headers.iter().cloned());
    let key = cache_key("headers", &serde_json::to_vec(&text).map_err(|why| why.to_string())?);
    if stored(database, input, &key, words::HEADERS, json!({"field":"headers"}), scratch, cancelled, |spool| {
        for (index, (name, value)) in text.iter().enumerate() {
            if index >= modeled {
                spool.text(
                    Cursor::new(name.as_bytes()),
                    json!({"field":"headers","entry":index,"header":name,"part":"name"}),
                    "native",
                    cancelled,
                )?;
            }
            spool.text(Cursor::new(value.as_bytes()), json!({"field":"headers","entry":index,"header":name,"part":"value"}), "native", cancelled)?;
        }
        Ok((
            true,
            json!({"complete":true,"scope":"all retained headers, modeled fields reconstructed; original MIME headers not retained are not inferred"}),
        ))
    })? && input.metadata.downloaded
    {
        complete |= words::HEADERS;
    }
    Ok(complete)
}

pub fn html(spool: &mut Spool, source: impl std::io::Read, location: Value, cancelled: &AtomicBool) -> Result<(String, u64)> {
    let parser = craft_content::html::HtmlText::default();
    let mut offset = 0u64;
    let mut preview = String::new();
    let mut preview_chars = 0usize;
    let mut input_bytes = 0u64;
    let mut append = |text: String| -> Result<()> {
        for ch in text.chars().take(8192usize.saturating_sub(preview_chars)) {
            preview.push(ch);
            preview_chars += 1;
        }
        let length = text.len() as u64;
        spool.record(Record { location: location.clone(), offset, text, derivation: Derivation::Native })?;
        offset = offset.checked_add(length).ok_or("HTML text source offset overflow")?;
        Ok(())
    };
    segments::utf8(source, cancelled, |chunk| {
        // A streaming tokenizer may retain one unfinished tag/attribute. Bound
        // that representation's input explicitly; raw retained HTML is indexed
        // separately through EOF and is not silently shortened by this limit.
        input_bytes = input_bytes.checked_add(chunk.len() as u64).ok_or("resourceLimit: HTML input size overflow")?;
        if input_bytes > craft_content::MAX_INPUT {
            return Err("resourceLimit: HTML text tokenization exceeds 64 MiB; complete retained source remains separately indexed".into());
        }
        append(parser.feed(chunk))
    })?;
    let (last, errors) = parser.finish();
    append(last)?;
    Ok((preview, errors))
}

pub fn index(
    database: &mut Database,
    input: Input,
    scratch: &Path,
    extractor: &super::extract::Extractor,
    cancelled: &AtomicBool,
    phase: Phase,
) -> Result<Completion> {
    let mut fields = headers(database, &input, scratch, cancelled)?;
    let mut snippet = None::<String>;
    let mut body_complete = input.metadata.downloaded;
    for (part, field) in [(Part::Text, "text"), (Part::Html, "htmlSource"), (Part::Hidden, "retainedHiddenText")] {
        let content = reference(&input.message, part)?;
        let key = format!("{}:{field}:{}:{}", super::input::EXTRACTOR_VERSION, hex(&content.sha256), input.metadata.downloaded);
        let complete = stored(database, &input, &key, words::BODY, json!({"field":field}), scratch, cancelled, |spool| {
            if !input.metadata.downloaded {
                return Ok((false, json!({"complete":false,"gaps":[{"code":"missing","detail":"the complete message has not been downloaded"}]})));
            }
            let preview = spool.text(input.open(part)?, json!({"field":field}), "native", cancelled)?;
            let preview = mailcraft_html::text_preview(&preview, 2000);
            if part == Part::Text {
                snippet = Some(preview.clone())
            }
            Ok((true, json!({"complete":true,"scope":"complete retained UTF-8 source through verified EOF","snippet":preview})))
        })?;
        if part == Part::Text
            && let Some((_, _, coverage)) = database.part_ready(&key)?
        {
            snippet = coverage.get("snippet").and_then(Value::as_str).map(ToOwned::to_owned);
        }
        body_complete &= complete;
        if part == Part::Html && content.len != 0 {
            let key = format!("{}:htmlText:{}:{}", super::input::EXTRACTOR_VERSION, hex(&content.sha256), input.metadata.downloaded);
            body_complete &= stored(database, &input, &key, words::BODY, json!({"field":"htmlText"}), scratch, cancelled, |spool| {
                if !input.metadata.downloaded {
                    return Ok((false, json!({"complete":false,"gaps":[{"code":"missing","detail":"HTML has not been downloaded"}]})));
                }
                let (preview, parse_errors) = html(spool, input.open(Part::Html)?, json!({"field":"htmlText","hiddenTextIncluded":true}), cancelled)?;
                let preview = mailcraft_html::text_preview(&preview, 2000);
                if snippet.as_ref().is_none_or(String::is_empty) {
                    snippet = Some(preview.clone())
                }
                Ok((
                    true,
                    json!({"complete":true,"scope":"complete retained HTML text, including hidden elements; script/style programs excluded from decoded text and retained in HTML source","htmlRecoveryEvents":parse_errors,"snippet":preview}),
                ))
            })?;
            if snippet.as_ref().is_none_or(String::is_empty)
                && let Some((_, _, coverage)) = database.part_ready(&key)?
            {
                snippet = coverage.get("snippet").and_then(Value::as_str).map(ToOwned::to_owned);
            }
        }
    }
    if body_complete {
        fields |= words::BODY
    }
    let mut attachments_complete = input.metadata.downloaded;
    let needs_attachments = matches!(phase, Phase::Body) && !input.message.attachments.is_empty();
    for (index, file) in input.message.attachments.iter().enumerate().filter(|_| !needs_attachments) {
        let content = reference(&input.message, Part::Attachment(index))?;
        let key = format!(
            "{}:attachment:{}:{}:{}",
            super::input::EXTRACTOR_VERSION,
            extractor.revision(),
            hex(&content.sha256),
            cache_key("type", format!("{}\n{}", file.mime, file.name.rsplit('.').next().unwrap_or_default()).as_bytes())
        );
        attachments_complete &= stored(
            database,
            &input,
            &key,
            words::ATTACHMENT_CONTENT,
            json!({"field":"attachmentContent","attachment":index,"name":file.name,"mime":file.mime,"sha256":hex(&content.sha256),"bytes":content.len}),
            scratch,
            cancelled,
            |spool| extractor.attachment(&input, index, scratch, spool, cancelled),
        )?;
    }
    if attachments_complete && !needs_attachments {
        fields |= words::ATTACHMENT_CONTENT
    }
    let complete = fields == words::ALL;
    let state = if complete {
        "indexed"
    } else if needs_attachments {
        "pendingAttachments"
    } else if !input.metadata.downloaded {
        "missing"
    } else {
        "partial"
    };
    // A cache hit must keep its persisted preview; absence of a new preview is
    // different from a message whose actual preview is empty.
    let snippet = snippet
        .or_else(|| database.row(input.message.id).ok().flatten().and_then(|row| row.get("snippet").and_then(Value::as_str).map(ToOwned::to_owned)))
        .unwrap_or_default();
    let _ = database.finish(
        input.message.id,
        &input.metadata.content_fp,
        fields,
        &snippet,
        state,
        (!complete).then_some("some retained sources are missing, failed or outside the explicitly reported extraction coverage"),
    )?;
    Ok(Completion { id: input.message.id, fingerprint: input.metadata.content_fp, needs_attachments })
}
