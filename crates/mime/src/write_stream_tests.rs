// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Streaming exports preserve complete content and stop on reader/write errors.

use std::io::{self, Cursor, Read, Write};
use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Body, ContentRef, Message};

use crate::{ContentPart, WriteOptions, parse, write_to};

const NONCE: &str = "0123456789abcdef0123456789abcdef";

fn open(message: &Message, part: ContentPart) -> io::Result<Box<dyn Read + '_>> {
    let bytes = match part {
        ContentPart::Text => message.body.text.as_bytes(),
        ContentPart::Html => message.body.html.as_deref().unwrap_or_default().as_bytes(),
        ContentPart::Attachment(index) => message.attachments.get(index).ok_or_else(|| io::Error::other("missing part"))?.data.as_slice(),
    };
    Ok(Box::new(Cursor::new(bytes)))
}

#[test]
fn streamed_complete_mime_reopens_with_bodies_headers_inline_ids_and_files() {
    let message = Message {
        from: Address::new("Avery Quinn", "avery@example.org"),
        reply_to: vec![Address::new("Private reply", "private_reply@aliases.example")],
        to: vec![Address::new("Blake", "blake@example.org")],
        subject: "A café receipt".into(),
        body: Body { text: "Hello\nAvery".into(), html: Some("<p>Hello <b>Avery</b></p><img src=\"cid:receipt\">".into()) },
        headers: vec![("X-Receipt-Id".into(), "receipt-123".into())],
        attachments: vec![
            Attachment {
                name: "receipt.png".into(),
                mime: "image/png".into(),
                inline: true,
                content_id: "receipt".into(),
                data: Arc::new(vec![0, 1, 2, 255]),
                ..Default::default()
            },
            Attachment::new("receipt.pdf", "application/pdf", b"%PDF-synthetic".to_vec()),
        ],
        ..Default::default()
    };
    let mut bytes = Vec::new();
    write_to(&message, &WriteOptions::default(), NONCE, &mut |part| open(&message, part), &mut bytes).unwrap();
    let source = String::from_utf8_lossy(&bytes);
    for kind in ["multipart/mixed", "multipart/alternative", "multipart/related"] {
        assert!(source.contains(kind));
    }
    let reopened = parse(&bytes);
    assert_eq!(reopened.subject, message.subject);
    assert_eq!(reopened.reply_to, message.reply_to);
    assert_eq!(reopened.body.text, message.body.text);
    assert_eq!(reopened.body.html, message.body.html);
    assert!(reopened.headers.contains(&("X-Receipt-Id".into(), "receipt-123".into())));
    assert_eq!(reopened.attachments.len(), message.attachments.len());
    for (actual, expected) in reopened.attachments.iter().zip(&message.attachments) {
        assert_eq!(actual.data, expected.data);
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.content_id, expected.content_id);
        assert_eq!(actual.inline, expected.inline);
    }
}

struct Count(u64);
impl Write for Count {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(bytes.len() <= 64 << 10);
        self.0 += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_large_external_attachment_is_streamed_without_loading_it_into_the_message() {
    let size = 20 << 20;
    let message = Message {
        body: Body { text: "A large file".into(), html: None },
        attachments: vec![Attachment {
            name: "large.bin".into(),
            mime: "application/octet-stream".into(),
            kept: Some(ContentRef { len: size, sha256: [0; 32] }),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut out = Count(0);
    write_to(
        &message,
        &WriteOptions::default(),
        NONCE,
        &mut |part| {
            if part == ContentPart::Attachment(0) { Ok(Box::new(io::repeat(42).take(size)) as Box<dyn Read>) } else { open(&message, part) }
        },
        &mut out,
    )
    .unwrap();
    assert!(out.0 > size * 4 / 3);
    assert!(message.attachments[0].data.is_empty());
}

#[test]
fn reader_failure_and_raw_message_boundary_conflict_never_return_success() {
    let attached = format!("From: a@example.org\r\n\r\n--=_mc_{NONCE}_0\r\n");
    let message = Message { attachments: vec![Attachment::new("letter.eml", "message/rfc822", attached.into_bytes())], ..Default::default() };
    assert!(write_to(&message, &WriteOptions::default(), NONCE, &mut |part| open(&message, part), &mut Count(0)).is_err());
    assert!(write_to(&message, &WriteOptions::default(), NONCE, &mut |_| Err(io::Error::other("missing retained content")), &mut Count(0)).is_err());
}

#[test]
fn text_line_end_canonicalization_handles_cr_lf_crlf_and_consecutive_cr() {
    let message = Message { body: Body { text: "one\rtwo\nthree\r\nfour\r\rfive\r".into(), html: None }, ..Default::default() };
    let mut bytes = Vec::new();
    write_to(&message, &WriteOptions::default(), NONCE, &mut |part| open(&message, part), &mut bytes).unwrap();
    assert_eq!(parse(&bytes).body.text, "one\ntwo\nthree\nfour\n\nfive\n");
}

struct ShortRead<R>(R);
impl<R: Read> Read for ShortRead<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = bytes.len().min(3);
        self.0.read(&mut bytes[..count])
    }
}

#[test]
fn delivery_status_and_attached_messages_keep_legal_encoding_and_exact_bytes() {
    let report = b"Reporting-MTA: dns; mx.example.org\r\n\r\nFinal-Recipient: rfc822; owner@example.org\r\nAction: failed\r\nStatus: 5.1.1\r\n";
    let attached = b"From: sender@example.org\r\nTo: owner@example.org\r\n\r\nBinary body: \xff\0\r\n";
    let message = Message {
        attachments: vec![
            Attachment::new("delivery.txt", "message/delivery-status", report.to_vec()),
            Attachment::new("original.eml", "message/rfc822", attached.to_vec()),
        ],
        ..Default::default()
    };
    let mut bytes = Vec::new();
    write_to(&message, &WriteOptions::default(), NONCE, &mut |part| Ok(Box::new(ShortRead(open(&message, part)?))), &mut bytes).unwrap();
    let wire = String::from_utf8_lossy(&bytes);
    assert!(wire.contains("Content-Transfer-Encoding: 7bit\r\nContent-Disposition: attachment; filename=\"delivery.txt\""));
    assert!(wire.contains("Content-Transfer-Encoding: binary\r\nContent-Disposition: attachment; filename=\"original.eml\""));
    let reopened = parse(&bytes);
    assert_eq!(reopened.attachments.len(), 2);
    for (actual, expected) in reopened.attachments.iter().zip(&message.attachments) {
        assert_eq!(actual.mime, expected.mime);
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.data, expected.data);
    }
}

#[test]
fn invalid_delivery_status_content_cannot_be_published_as_valid_7bit() {
    for data in [
        b"Bad: \xff\r\n".to_vec(),
        b"Bad: \0\r\n".to_vec(),
        b"Bad: bare\n".to_vec(),
        b"Bad: bare\rX".to_vec(),
        b"Bad: final\r".to_vec(),
        vec![b'x'; 999],
    ] {
        let message = Message { attachments: vec![Attachment::new("delivery.txt", "message/delivery-status", data)], ..Default::default() };
        let error = write_to(&message, &WriteOptions::default(), NONCE, &mut |part| Ok(Box::new(ShortRead(open(&message, part)?))), &mut Count(0))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
