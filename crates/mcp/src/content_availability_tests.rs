// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A full-message read never passes headers or known missing content off as full.

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::Headless;
use crate::tools::{Policy, call_tool};

#[test]
fn full_message_reads_refuse_partial_or_failed_content_without_returning_its_preview() {
    for unavailable in 0..4 {
        let mut session = Session::new(mailcraft_engine::sample::mailbox(1_791_993_600, 0));
        let id = *session.mailbox.messages.keys().next().unwrap();
        let message = std::sync::Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap());
        message.body.text = "Only a cached preview, never a complete message.".into();
        match unavailable {
            0 => message.unfetched = true,
            1 => message.incomplete = true,
            2 => message.not_downloaded = 20_000_000,
            _ => message.kept.get_or_insert_with(Default::default).failed = true,
        }
        let mut backend = Headless::new(session, Vec::new());
        let result = call_tool(&mut backend, &Policy::default(), "read_message", &json!({"id": id}));
        assert!(result.is_error);
        let text = result.content[0]["text"].as_str().unwrap();
        assert!(text.contains("complete content"), "{text}");
        assert!(!text.contains("Only a cached preview"));
        let metadata = call_tool(&mut backend, &Policy::default(), "execute", &json!({"command": "mail.get", "params": {"id": id}}));
        assert!(!metadata.is_error);
        let value: Value = serde_json::from_str(metadata.content[0]["text"].as_str().unwrap()).unwrap();
        assert!(value["content"].is_object());
    }
}

#[test]
fn complete_message_reads_keep_full_content_and_report_its_availability() {
    let session = Session::new(mailcraft_engine::sample::mailbox(1_791_993_600, 0));
    let id = *session.mailbox.messages.keys().next().unwrap();
    let mut backend = Headless::new(session, Vec::new());
    let result = call_tool(&mut backend, &Policy::default(), "read_message", &json!({"id": id}));
    assert!(!result.is_error);
    let value: Value = serde_json::from_str(result.content[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(value["content"], json!({"unfetched": false, "incomplete": false, "notDownloaded": 0, "readFailed": false}));
    assert!(!value["body"].as_str().unwrap().is_empty());
}
