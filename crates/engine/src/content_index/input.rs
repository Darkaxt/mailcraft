// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One current message per extraction job. Queue entries are ids; they never
//! pin a mailbox-wide snapshot of loaded message bodies and attachments.

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use mailcraft_model::{ContentRef, Message};
use serde_json::{Value, json};

use super::db::Metadata;
use crate::{Part, Session, io::ContentSource};

type Result<T> = std::result::Result<T, String>;
pub const EXTRACTOR_VERSION: &str = "craft-content-1-ocr2";

pub struct Input {
    pub message: Arc<Message>,
    pub metadata: Metadata,
    pub path: PathBuf,
    pub source: Option<Arc<dyn ContentSource>>,
}

struct HashWriter(ring::digest::Context);

impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn hash_json(value: &Value) -> Result<String> {
    let mut hash = HashWriter(ring::digest::Context::new(&ring::digest::SHA256));
    serde_json::to_writer(&mut hash, value).map_err(|why| why.to_string())?;
    Ok(hex(hash.0.finish().as_ref()))
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(char::from(*DIGITS.get(usize::from(byte >> 4)).unwrap_or(&b'0')));
        out.push(char::from(*DIGITS.get(usize::from(byte & 15)).unwrap_or(&b'0')));
    }
    out
}

fn parts(message: &Message, part: Part) -> Result<(Option<ContentRef>, &[u8])> {
    let kept = message.kept.as_deref();
    Ok(match part {
        Part::Text => (
            kept.and_then(|kept| kept.text).filter(|_| message.body.text.is_empty() || kept.is_some_and(|kept| kept.failed)),
            message.body.text.as_bytes(),
        ),
        Part::Html => (
            kept.and_then(|kept| kept.html).filter(|_| message.body.html.is_none() || kept.is_some_and(|kept| kept.failed)),
            message.body.html.as_deref().unwrap_or_default().as_bytes(),
        ),
        Part::Hidden => (
            kept.and_then(|kept| kept.hidden).filter(|_| message.hidden_text.is_empty() || kept.is_some_and(|kept| kept.failed)),
            message.hidden_text.as_bytes(),
        ),
        Part::Attachment(index) => {
            let file = message.attachments.get(index).ok_or("the attachment no longer exists")?;
            (file.kept.filter(|_| file.data.is_empty()), file.data.as_slice())
        }
    })
}

pub fn reference(message: &Message, part: Part) -> Result<ContentRef> {
    let (kept, bytes) = parts(message, part)?;
    if let Some(kept) = kept {
        return Ok(kept);
    }
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut sha256 = [0u8; 32];
    for (target, byte) in sha256.iter_mut().zip(digest.as_ref()) {
        *target = *byte;
    }
    Ok(ContentRef { sha256, len: bytes.len() as u64 })
}

pub fn metadata(session: &Session, message: &Message, extraction_revision: &str) -> Result<Metadata> {
    let text = reference(message, Part::Text)?;
    let html = reference(message, Part::Html)?;
    let hidden = reference(message, Part::Hidden)?;
    let attachments = message
        .attachments
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let content = reference(message, Part::Attachment(index))?;
            Ok(json!({"name":file.name,"mime":file.mime,"inline":file.inline,"contentId":file.content_id,
            "sha256":hex(&content.sha256),"bytes":content.len}))
        })
        .collect::<Result<Vec<_>>>()?;
    let content_fp = hash_json(&json!({"version":EXTRACTOR_VERSION,"extractionRevision":extraction_revision,"subject":message.subject,
        "from":message.from,"to":message.to,"cc":message.cc,"bcc":message.bcc,"replyTo":message.reply_to,
        "messageId":message.message_id,"inReplyTo":message.in_reply_to,"references":message.references,
        "date":message.date,"headers":message.headers,"text":hex(&text.sha256),"html":hex(&html.sha256),
        "hidden":hex(&hidden.sha256),"attachments":attachments,"unfetched":message.unfetched,
        "incomplete":message.incomplete,"notDownloaded":message.not_downloaded,"failed":message.kept.as_ref().is_some_and(|kept| kept.failed)}))?;
    let downloaded = !message.unfetched && !message.incomplete && message.not_downloaded == 0;
    let row = json!({"id":message.id,"account":message.account,"folder":message.folder,
        "filingSourceAccount":(message.filing_source_account!=0).then_some(message.filing_source_account),
        "filingSourceAlias":message.filing_source_alias,"filingOverride":message.filing_override,
        "simpleLogin":message.simple_login(),"from":message.from.full(),"to":mailcraft_model::format_list(&message.to),
        "subject":message.subject,"date":mailcraft_time::format_iso_utc(message.date),
        "received":session.local(crate::list::when(message)).iso(),"read":message.read,"flag":message.flag,
        "importance":message.importance,"categories":message.categories,"gmailCategory":message.gmail_category.map(|category|category.code()),
        "attachments":message.listed_attachments().map(|(_,file)| file.name.as_str()).collect::<Vec<_>>()});
    let metadata_fp = hash_json(&row)?;
    Ok(Metadata {
        id: message.id,
        account: message.account,
        folder: message.folder,
        content_fp,
        metadata_fp,
        row,
        snippet: String::new(),
        downloaded,
        attachments: message.attachments.len() as u64,
        complete_fields: 0,
        state: if downloaded { "pending".into() } else { "missing".into() },
        problem: None,
    })
}

/// Conservative serialized size without allocating another copy of long names
/// or headers. One oversized retained metadata row is processed on its own.
pub fn metadata_bytes(metadata: &Metadata) -> usize {
    fn size(value: &Value) -> usize {
        match value {
            Value::String(text) => text.len().saturating_mul(6).saturating_add(2),
            Value::Array(items) => items.iter().fold(2usize, |bytes, item| bytes.saturating_add(size(item)).saturating_add(1)),
            Value::Object(items) => items.iter().fold(2usize, |bytes, (name, value)| {
                bytes.saturating_add(name.len().saturating_mul(6)).saturating_add(size(value)).saturating_add(4)
            }),
            _ => 32,
        }
    }
    size(&metadata.row).saturating_add(1024)
}

impl Input {
    pub fn new(session: &Session, id: u64, extraction_revision: &str) -> Result<Self> {
        let message = session.mailbox.messages.get(&id).cloned().ok_or("the message no longer exists")?;
        let metadata = metadata(session, &message, extraction_revision)?;
        let path = session.path.clone().ok_or("a persistent index requires a native mailbox path")?;
        Ok(Self { message, metadata, path, source: session.source() })
    }

    pub fn open(&self, part: Part) -> Result<Box<dyn Read + '_>> {
        let (reference, bytes) = parts(&self.message, part)?;
        if let Some(reference) = reference {
            let source = self.source.as_ref().ok_or("the retained content has no readable source")?;
            source.open(&self.path, &reference).map(|reader| reader as Box<dyn Read>)
        } else {
            Ok(Box::new(Cursor::new(bytes)))
        }
    }
}
