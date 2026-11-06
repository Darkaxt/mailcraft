// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Resident accounting must not load mail, count shared attachment buffers twice,
//! expose content or change the mailbox while diagnosing native memory use.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mailcraft_model::{Attachment, ContentRef, KeptBody, Mailbox, Message};
use serde_json::json;

use crate::Session;

struct NotRead(Arc<AtomicUsize>);

impl crate::io::ContentSource for NotRead {
    fn fetch(&self, _: &std::path::Path, _: &ContentRef) -> Result<Arc<Vec<u8>>, String> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Err("diagnostic must not fetch this synthetic content".into())
    }
}

#[test]
fn resident_accounting_is_read_only_and_counts_shared_attachment_capacity_once() {
    let mut mailbox = Mailbox::with_local_account("Synthetic", "synthetic@example.org");
    let inbox = mailbox.inbox().unwrap();
    let mut text = String::with_capacity(4096);
    text.push_str("private synthetic body");
    let text_len = text.len();
    let mut bytes = Vec::with_capacity(8192);
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    let shared = Attachment::new("private synthetic attachment", "application/octet-stream", bytes);
    let first = Message {
        folder: inbox,
        subject: "private synthetic subject".into(),
        body: mailcraft_model::Body { text, html: None },
        attachments: vec![shared.clone()],
        ..Default::default()
    };
    mailbox.add_message(first).unwrap();
    let kept = ContentRef { sha256: [7; 32], len: 128 };
    let second = Message {
        folder: inbox,
        kept: Some(Box::new(KeptBody { text: Some(kept), ..Default::default() })),
        attachments: vec![shared, Attachment { kept: Some(kept), ..Default::default() }],
        ..Default::default()
    };
    let second = mailbox.add_message(second).unwrap();
    let mut session = Session::new(mailbox);
    let reads = Arc::new(AtomicUsize::new(0));
    session.content_source = Some(Arc::new(NotRead(reads.clone())));
    session.path = Some(std::env::temp_dir().join("synthetic-memory-not-opened.mailcraft"));
    let (rev, dirty, resident) = (session.rev(), session.dirty, session.resident_bytes());
    let ordinary = session.run("file.info", &json!({})).unwrap();
    assert!(ordinary.get("memory").is_none());
    let report = session.run("file.info", &json!({"memory": true})).unwrap();
    let memory = &report["memory"];
    assert_eq!(memory["mailboxMessages"], 2);
    assert_eq!(memory["bodyTextBytes"], text_len);
    assert_eq!(memory["bodyStringCapacityBytes"], 4096);
    assert_eq!(memory["referencedAttachmentBytes"], 8);
    assert_eq!(memory["uniqueAttachmentCapacityBytes"], 8192);
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    assert!(session.mailbox.message(second).unwrap().body_is_kept());
    assert_eq!((session.rev(), session.dirty, session.resident_bytes()), (rev, dirty, resident));
    assert!(!memory.to_string().contains("private synthetic"));
}
