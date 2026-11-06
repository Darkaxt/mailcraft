// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only resident payload/capacity accounting. These are attribution counters,
//! not a process heap total: shared buffers, allocator slack and native services
//! have different ownership, and a diagnostic must not fetch content to count it.

use std::collections::HashSet;
use std::mem::size_of;
use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Message};
use serde_json::{Value, json};

use crate::Session;

fn strings(values: &[String]) -> u64 {
    values.iter().fold((values.len() as u64).saturating_mul(size_of::<String>() as u64), |n, s| n.saturating_add(s.capacity() as u64))
}

fn address(value: &Address) -> u64 {
    (value.name.capacity() as u64).saturating_add(value.email.capacity() as u64)
}

fn addresses(values: &Vec<Address>) -> u64 {
    values.iter().fold((values.capacity() as u64).saturating_mul(size_of::<Address>() as u64), |n, a| n.saturating_add(address(a)))
}

pub(crate) fn inspect(session: &Session) -> Value {
    let (mut text, mut html, mut hidden, mut body_capacity, mut attachments, mut unique_attachments, mut metadata) =
        (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut seen = HashSet::new();
    for message in session.mailbox.messages.values() {
        text = text.saturating_add(message.body.text.len() as u64);
        html = html.saturating_add(message.body.html.as_ref().map_or(0, String::len) as u64);
        hidden = hidden.saturating_add(message.hidden_text.len() as u64);
        body_capacity = body_capacity
            .saturating_add(message.body.text.capacity() as u64)
            .saturating_add(message.body.html.as_ref().map_or(0, String::capacity) as u64)
            .saturating_add(message.hidden_text.capacity() as u64);
        for value in [&message.message_id, &message.in_reply_to, &message.subject, &message.placed, &message.held] {
            metadata = metadata.saturating_add(value.capacity() as u64);
        }
        metadata = metadata.saturating_add(address(&message.from));
        for values in [&message.to, &message.cc, &message.bcc, &message.reply_to] {
            metadata = metadata.saturating_add(addresses(values));
        }
        for values in [&message.references, &message.categories, &message.warnings_dismissed] {
            metadata = metadata.saturating_add(strings(values));
        }
        metadata = metadata.saturating_add((message.headers.capacity() as u64).saturating_mul(size_of::<(String, String)>() as u64));
        for (name, value) in &message.headers {
            metadata = metadata.saturating_add(name.capacity() as u64).saturating_add(value.capacity() as u64);
        }
        metadata = metadata.saturating_add((message.attachments.capacity() as u64).saturating_mul(size_of::<Attachment>() as u64));
        for attachment in &message.attachments {
            metadata = metadata
                .saturating_add(attachment.name.capacity() as u64)
                .saturating_add(attachment.mime.capacity() as u64)
                .saturating_add(attachment.content_id.capacity() as u64);
            attachments = attachments.saturating_add(attachment.data.len() as u64);
            if seen.insert(Arc::as_ptr(&attachment.data)) {
                unique_attachments = unique_attachments.saturating_add(attachment.data.capacity() as u64);
            }
        }
        if let Some(server) = &message.server {
            for value in [&server.id, &server.folder, &server.change_key, &server.thread] {
                metadata = metadata.saturating_add(value.capacity() as u64);
            }
            metadata = metadata.saturating_add(strings(&server.categories));
        }
    }
    let result = json!({
        "mailboxMessages": session.mailbox.messages.len(),
        "shallowMessageStructBytes": (session.mailbox.messages.len() as u64).saturating_mul(size_of::<Message>() as u64),
        "commonMetadataCapacityBytes": metadata,
        "bodyTextBytes": text, "bodyHtmlBytes": html, "hiddenTextBytes": hidden,
        "bodyStringCapacityBytes": body_capacity,
        "referencedAttachmentBytes": attachments, "uniqueAttachmentCapacityBytes": unique_attachments,
        "sessionResidentBytes": session.resident_bytes(), "sessionLoadedBudgetBytes": session.loaded_budget,
        "undoSteps": session.history.len(), "redoSteps": session.redo.len(),
        "wordIndexConfigured": session.index_words, "wordIndex": session.word_memory_profile(),
        "accounting": "Resident data only; no content is loaded. Capacities are lower-bound attribution, not total heap. Session-resident and shared content-cache counters may overlap. Allocator/container slack, other mailbox structures, native services and renderer allocations are excluded."
    });
    #[cfg(not(target_arch = "wasm32"))]
    let result = {
        let mut result = result;
        result["startupBodyBudgetBytes"] = json!(crate::store::body_budget());
        result["contentCacheBytes"] = json!(crate::store::cached_bytes());
        result["contentCacheBudgetBytes"] = json!(crate::store::CACHE_MAX);
        result
    };
    result
}
