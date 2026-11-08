// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Synthetic native source/export workflows, including damaged retained data.

use std::io::{Cursor, Read};
use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Body, FolderKind, KeptBody, Message};
use serde_json::json;

use crate::{Session, io::ContentSource, store};

fn fixture() -> (Session, u64) {
    let mut session = Session::empty();
    let account = session.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    let folder = session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let id = session
        .mailbox
        .add_message(Message {
            folder,
            account,
            from: Address::new("Sender", "sender@example.org"),
            to: vec![Address::new("Owner", "owner@example.org")],
            bcc: vec![Address::new("Known blind recipient", "blind@example.org")],
            reply_to: vec![Address::new("Alias reply", "private_reply@aliases.example")],
            subject: "A retained café receipt".into(),
            message_id: "receipt@example.org".into(),
            body: Body {
                text: "Hello\nReceipt".into(),
                html: Some(
                    "<html><body><p>Hello <b>café</b></p><script>do_not_execute()</script><img src=\"https://tracker.example/pixel\"></body></html>"
                        .into(),
                ),
            },
            headers: vec![("X-Receipt-Id".into(), "receipt-123".into())],
            attachments: vec![Attachment::new("receipt.pdf", "application/pdf", b"%PDF-synthetic-complete".to_vec())],
            ..Default::default()
        })
        .unwrap();
    session.mailbox_changed();
    (session, id)
}

fn ready(session: &mut Session, command: &str, id: u64) -> u64 {
    let opened = session.run(command, &json!({"id":id})).unwrap();
    assert_eq!(opened["state"], "pending");
    let token = opened["token"].as_u64().unwrap();
    session.wait_source_exports();
    assert!(!session.source_export_busy());
    token
}

fn close(session: &mut Session, command: &str, id: u64, token: u64) {
    assert_eq!(session.run(command, &json!({"id":id,"token":token,"close":true})).unwrap()["state"], "closed");
}

#[test]
fn formatted_source_has_lossless_continuation_and_is_a_read_only_snapshot() {
    let (mut session, id) = fixture();
    let original = serde_json::to_value(&session.mailbox).unwrap();
    let token = ready(&mut session, "mail.sourcePage", id);
    let mut all = String::new();
    let mut offset = 0;
    loop {
        let answer = session.run("mail.sourcePage", &json!({"id":id,"token":token,"offset":offset,"limit":7})).unwrap();
        assert_eq!(answer["representation"], "visibleText");
        assert_eq!(answer["provenance"], "decodedBody");
        assert_eq!(answer["format"], "html");
        assert_eq!(answer["formatted"], true);
        all.push_str(answer["source"].as_str().unwrap());
        if answer["eof"] == true {
            break;
        }
        let next = answer["nextOffset"].as_u64().unwrap();
        assert!(next > offset);
        offset = next;
    }
    assert!(all.contains("<script>do_not_execute()</script>"));
    assert!(all.contains("\n  <body>\n"));
    assert!(all.contains("café"));
    assert!(all.contains("https://tracker.example/pixel"));
    assert_eq!(serde_json::to_value(&session.mailbox).unwrap(), original);
    assert!(session.run("mail.sourcePage", &json!({"id":id+1,"token":token})).is_err());
    assert!(session.run("mail.exportEml", &json!({"id":id,"token":token})).is_err());
    close(&mut session, "mail.sourcePage", id, token);
    assert!(session.run("mail.sourcePage", &json!({"id":id,"token":token})).is_err());
}

#[test]
fn complete_eml_chunks_reopen_all_known_headers_bodies_and_attachments() {
    let (mut session, id) = fixture();
    let original = session.mailbox.messages.get(&id).unwrap().clone();
    let token = ready(&mut session, "mail.exportEml", id);
    let mut bytes = Vec::new();
    let mut offset = 0;
    let digest = loop {
        let value = session.run("mail.exportEml", &json!({"id":id,"token":token,"offset":offset,"limit":31})).unwrap();
        assert_eq!(value["originalMimeAvailable"], false);
        assert_eq!(value["provenance"], "reconstructed");
        bytes.extend(crate::cmd::base64_decode(value["data"].as_str().unwrap()).unwrap());
        if value["eof"] == true {
            assert_eq!(bytes.len() as u64, value["bytes"].as_u64().unwrap());
            break value["sha256"].as_str().unwrap().to_string();
        }
        offset = value["nextOffset"].as_u64().unwrap();
    };
    assert_eq!(digest, store::Digest::of(&bytes).hex());
    let reopened = mailcraft_mime::parse(&bytes);
    assert_eq!(reopened.subject, original.subject);
    assert_eq!(reopened.from, original.from);
    assert_eq!(reopened.bcc, original.bcc);
    assert_eq!(reopened.reply_to, original.reply_to);
    assert_eq!(reopened.body, original.body);
    assert!(reopened.headers.contains(&("X-Receipt-Id".into(), "receipt-123".into())));
    assert_eq!(reopened.attachments[0].name, "receipt.pdf");
    assert_eq!(reopened.attachments[0].data, original.attachments[0].data);
    close(&mut session, "mail.exportEml", id, token);
}

#[test]
fn source_validation_and_plain_text_labels_are_explicit() {
    let (mut session, id) = fixture();
    for params in [
        json!({"id":id,"limit":0}),
        json!({"id":id,"limit":65537}),
        json!({"id":id,"offset":-1}),
        json!({"id":id,"offset":1}),
        json!({"id":id,"close":"yes"}),
        json!({"id":id,"path":"ignored"}),
    ] {
        assert!(session.run("mail.sourcePage", &params).is_err(), "{params}");
        assert!(!session.source_export_busy());
    }
    let message = Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap());
    message.body = Body { text: "Plain <b>text</b>\u{202e} remains text".into(), html: None };
    let token = ready(&mut session, "mail.sourcePage", id);
    let answer = session.run("mail.sourcePage", &json!({"id":id,"token":token})).unwrap();
    assert_eq!(answer["format"], "text");
    assert_eq!(answer["formatted"], false);
    assert_eq!(answer["source"], "Plain <b>text</b><U+202E> remains text");
    close(&mut session, "mail.sourcePage", id, token);
}

fn blob_path(mailbox: &std::path::Path, bytes: &[u8]) -> std::path::PathBuf {
    let hash = store::Digest::of(bytes).hex();
    store::blob_dir(mailbox).join(&hash[..2]).join(hash)
}

#[test]
fn retained_streams_validate_length_and_hash_without_loading_the_model() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mailbox.mailcraft");
    let bytes = vec![42; 2 << 20];
    let reference = store::stash(&path, &store::Digest::of(&bytes), &bytes).unwrap();
    let mut reader = store::Content.open(&path, &reference).unwrap();
    let mut count = 0;
    let mut block = [0u8; 8192];
    loop {
        let read = reader.read(&mut block).unwrap();
        if read == 0 {
            break;
        }
        assert!(block[..read].iter().all(|byte| *byte == 42));
        count += read;
    }
    assert_eq!(count, bytes.len());
    drop(reader);
    // Same-size corruption must fail at EOF, even though opening/metadata work.
    std::fs::write(blob_path(&path, &bytes), vec![43; bytes.len()]).unwrap();
    let mut reader = store::Content.open(&path, &reference).unwrap();
    assert!(std::io::copy(&mut reader, &mut std::io::sink()).is_err());
    drop(reader);
    std::fs::write(blob_path(&path, &bytes), b"short").unwrap();
    assert!(store::Content.open(&path, &reference).is_err());
}

#[test]
fn missing_or_damaged_content_cannot_be_exported_as_a_complete_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("mailbox.mailcraft");
    let (mut session, id) = fixture();
    session.path = Some(path.clone());
    let data = b"Complete retained HTML";
    let reference = store::stash(&path, &store::Digest::of(data), data).unwrap();
    let message = Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap());
    message.body.html = None;
    message.kept = Some(Box::new(KeptBody { html: Some(reference), ..Default::default() }));
    std::fs::write(blob_path(&path, data), b"Corrupt retained HTML!").unwrap();
    let token = ready(&mut session, "mail.exportEml", id);
    assert!(session.run("mail.exportEml", &json!({"id":id,"token":token})).is_err());
    let destination = directory.path().join("existing.eml");
    std::fs::write(&destination, b"prior intact file").unwrap();
    assert!(session.run("file.exportMessage", &json!({"id":id,"path":destination.to_string_lossy()})).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"prior intact file");
    close(&mut session, "mail.exportEml", id, token);
}

#[test]
fn prepared_export_and_atomic_replacement_preserve_whole_file_or_prior_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("saved.eml");
    std::fs::write(&path, b"old").unwrap();
    assert!(store::write_atomic_from(&path, &mut Cursor::new(b"short"), 9).is_err());
    assert!(store::write_atomic_from(&path, &mut Cursor::new(b"too long"), 3).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"old");
    let (mut session, id) = fixture();
    let token = ready(&mut session, "mail.exportEml", id);
    let result = session.run("file.exportMessage", &json!({"id":id,"token":token,"path":path.to_string_lossy()})).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(result["sha256"], store::Digest::of(&bytes).hex());
    assert_eq!(mailcraft_mime::parse(&bytes).subject, "A retained café receipt");
    #[cfg(windows)]
    assert_eq!(std::fs::read(crate::mark::zone_stream(&path)).unwrap(), crate::mark::ZONE_IDENTIFIER);
    assert!(session.run("mail.exportEml", &json!({"id":id,"token":token})).is_err());
}

#[test]
fn closing_pending_work_waits_for_exit_and_pool_never_evicts_an_owned_reader() {
    let (mut session, id) = fixture();
    let mut tokens = Vec::new();
    for _ in 0..4 {
        tokens.push(ready(&mut session, "mail.sourcePage", id));
    }
    assert!(session.run("mail.sourcePage", &json!({"id":id})).is_err());
    assert_eq!(session.run("mail.sourcePage", &json!({"id":id,"token":tokens[0]})).unwrap()["state"], "ready");
    for token in tokens {
        close(&mut session, "mail.sourcePage", id, token);
    }
    let token = session.run("mail.sourcePage", &json!({"id":id})).unwrap()["token"].as_u64().unwrap();
    let value = session.run("mail.sourcePage", &json!({"id":id,"token":token,"close":true})).unwrap();
    session.wait_source_exports();
    if value["state"] == "closing" {
        close(&mut session, "mail.sourcePage", id, token);
    }
    assert!(!session.source_export_busy());
}

#[test]
fn native_whole_data_response_refuses_large_message_but_chunked_export_remains_complete() {
    let (mut session, id) = fixture();
    Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap()).attachments.push(Attachment::new(
        "large.bin",
        "application/octet-stream",
        vec![42; 4 << 20],
    ));
    assert!(session.run("file.exportMessage", &json!({"id":id})).is_err());
    let token = ready(&mut session, "mail.exportEml", id);
    assert!(session.run("file.exportMessage", &json!({"id":id,"token":token})).is_err());
    let answer = session.run("mail.exportEml", &json!({"id":id,"token":token,"limit":65536})).unwrap();
    assert!(answer["bytes"].as_u64().unwrap() > 4 << 20);
    assert_eq!(answer["eof"], false);
    assert!(answer["data"].as_str().unwrap().len() < 90000);
    close(&mut session, "mail.exportEml", id, token);
}

#[test]
fn another_mailbox_opening_revokes_old_tokens_even_when_message_ids_are_reused() {
    let (mut session, id) = fixture();
    let token = ready(&mut session, "mail.sourcePage", id);
    session.run("file.new", &json!({"sample":true})).unwrap();
    assert!(session.run("mail.sourcePage", &json!({"id":id,"token":token})).is_err());
    let replacement = *session.mailbox.messages.keys().next().unwrap();
    let next = ready(&mut session, "mail.sourcePage", replacement);
    assert!(next > token);
    close(&mut session, "mail.sourcePage", replacement, next);
}
