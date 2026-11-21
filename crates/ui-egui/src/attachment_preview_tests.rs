// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Preview Pane and what an attachment's preview shows (v0.12.14): which preview a file gets,
//! the bounds on what is read here, the pane's switch, a file of a message being written, a file
//! that is never previewed, and the files a send question lists.

use std::sync::{Arc, mpsc};

use egui::PointerButton;
use mailcraft_model::Attachment;
use serde_json::json;

use super::*;
use crate::people_view::testing::{self, click};
use crate::reading::attachment_key;

/// A message of the sample mailbox that carries files.
const A: u64 = 13;

/// Files go to the preview their type has: by the name's extension first, then the MIME type;
/// a type with no preview has none (v0.12.14).
#[test]
fn each_file_type_gets_its_own_preview() {
    assert_eq!(kind_of("notes.txt", "application/octet-stream"), Kind::Text { mono: false });
    assert_eq!(kind_of("server.LOG", ""), Kind::Text { mono: true });
    assert_eq!(kind_of("data.csv", ""), Kind::Table(b','));
    assert_eq!(kind_of("data.tsv", ""), Kind::Table(b'\t'));
    assert_eq!(kind_of("page.htm", ""), Kind::Html);
    assert_eq!(kind_of("mail.eml", ""), Kind::Message);
    assert_eq!(kind_of("files.zip", ""), Kind::Archive);
    assert_eq!(kind_of("photo.JPG", ""), Kind::Picture);
    assert_eq!(kind_of("scan.tiff", ""), Kind::Pages("tif"));
    assert_eq!(kind_of("report.docx", ""), Kind::Pages("docx"));
    assert_eq!(kind_of("sheet.xlsx", ""), Kind::Pages("xlsx"));
    assert_eq!(kind_of("unnamed", "application/pdf"), Kind::Pages("pdf"));
    assert_eq!(kind_of("unnamed", "message/rfc822"), Kind::Message);
    assert_eq!(kind_of("setup.exe", "application/x-msdownload"), Kind::Unsupported);
    assert_eq!(kind_of("slides.ppt", ""), Kind::Unsupported);
}

/// A long text file shows its start, cut where a character ends; UTF-16 is read by its byte
/// order mark, and other text that isn't UTF-8 as Windows-1252 (v0.12.14).
#[test]
fn a_text_file_shows_its_start_in_its_own_character_set() {
    let long = "é".repeat(MAX_TEXT_BYTES);
    let (text, cut) = text_of(long.as_bytes(), MAX_TEXT_BYTES + 1);
    assert!(cut);
    assert_eq!(text.chars().count(), MAX_TEXT_BYTES / 2);
    assert_eq!(text_of(b"\xEF\xBB\xBFhello", 100), ("hello".to_string(), false));
    assert_eq!(text_of(b"\xFF\xFEh\0i\0", 100).0, "hi");
    assert_eq!(text_of(b"caf\xE9", 100).0, "café");
}

/// CSV is read with its quotes, at most so many rows, and says when there were more (v0.12.14).
#[test]
fn csv_rows_are_read_with_their_quotes_within_bounds() {
    let (rows, more) = table_of("name,note\r\nAda,\"one, two\"\nBob,\"say \"\"hi\"\"\"\n", ',');
    assert_eq!(rows, vec![vec!["name", "note"], vec!["Ada", "one, two"], vec!["Bob", "say \"hi\""]]);
    assert!(!more);
    let many = "x\n".repeat(MAX_ROWS + 5);
    let (rows, more) = table_of(&many, ',');
    assert_eq!(rows.len(), MAX_ROWS);
    assert!(more);
}

/// A ZIP file lists its files from its central directory, unpacking nothing; what isn't one is
/// not listed (v0.12.14).
#[test]
fn a_zip_file_lists_its_files_without_unpacking_them() {
    let mut zip = Vec::new();
    for (name, size) in [("a.txt", 5u32), ("dir/b.pdf", 70_000)] {
        zip.extend_from_slice(b"PK\x01\x02");
        zip.extend_from_slice(&[0; 20]);
        zip.extend_from_slice(&size.to_le_bytes());
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&[0; 16]);
        zip.extend_from_slice(name.as_bytes());
    }
    let directory = zip.len() as u32;
    zip.extend_from_slice(b"PK\x05\x06");
    zip.extend_from_slice(&[0; 6]);
    zip.extend_from_slice(&2u16.to_le_bytes());
    zip.extend_from_slice(&directory.to_le_bytes());
    zip.extend_from_slice(&0u32.to_le_bytes());
    zip.extend_from_slice(&[0; 2]);
    let (entries, total) = entries_of(&zip).unwrap();
    assert_eq!(entries, vec![("a.txt".to_string(), 5), ("dir/b.pdf".to_string(), 70_000)]);
    assert_eq!(total, 2);
    assert!(entries_of(b"not a zip file at all, not even close").is_none());
}

/// The Preview Pane is off at first; its switch is kept for the next run with its width, and a
/// width that makes no sense is put right (v0.12.14).
#[test]
fn the_preview_pane_is_off_at_first_and_its_switch_is_kept() {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    assert!(!app.session.view.preview_pane);
    assert_eq!(app.execute("view.previewPane", json!({})).unwrap(), json!({"value": true}));
    app.ui.preview_pane_width = f32::NAN;
    let mut saved = app.prefs();
    assert!(saved.view.preview_pane);
    saved.view.preview_pane = true;
    app.session.view.preview_pane = false;
    app.apply_prefs(saved);
    assert!(app.session.view.preview_pane);
    assert_eq!(app.ui.preview_pane_width, 380.0);
    h.run_steps(4);
    assert!(h.state().hit.rect("attachmentPreviewPane", 0).is_some());
}

/// With the Preview Pane on, one click on a text file shows it in the pane, beside the message
/// (no helper, nothing installed), and the pane's close takes it away (v0.12.14).
#[test]
fn with_the_pane_on_a_click_shows_a_text_file_beside_the_message() {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.session.mailbox.message_mut(A).unwrap().attachments = vec![Attachment::new("notes.txt", "text/plain", b"hello pane".to_vec())];
    app.execute("view.previewPane", json!({"value": true})).unwrap();
    app.run("mail.select", json!({"id": A})).unwrap();
    h.run_steps(5);
    let at = h.state().hit.rect_in(0, "attachment", attachment_key(A, 0)).unwrap().center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(30);
    assert!(h.state().hit.rect("attachmentPreviewText", A).is_some());
    assert!(h.state().hit.rect("attachmentPreviewBack", A).is_none(), "the message stays in its place");
    let close = h.state().hit.rect("attachmentPreviewClose", A).unwrap().center();
    click(&mut h, close, PointerButton::Primary, 1);
    h.run_steps(2);
    assert!(h.state().hit.rect("attachmentPreviewText", A).is_none());
}

/// With nothing turned on (no Preview Pane, no PdfCraft or PhotoCraft), a click on a file of a
/// message read or being written shows it in place of the body, with a way back (v0.12.25).
#[test]
fn a_click_previews_with_nothing_turned_on() {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    assert!(!app.session.view.preview_pane && !app.ui.attachment_previews);
    app.session.mailbox.message_mut(A).unwrap().attachments = vec![Attachment::new("notes.txt", "text/plain", b"hello".to_vec())];
    app.run("mail.select", json!({"id": A})).unwrap();
    h.run_steps(5);
    let at = h.state().hit.rect_in(0, "attachment", attachment_key(A, 0)).unwrap().center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(30);
    assert!(h.state().hit.rect("attachmentPreviewText", A).is_some() && h.state().hit.rect("attachmentPreviewBack", A).is_some());
    let app = h.state_mut();
    app.execute("mail.new", json!({"subject": "Notes"})).unwrap();
    app.execute("compose.attach", json!({"name": "notes.txt", "data": "aGVsbG8gd29ybGQ="})).unwrap();
    let compose = app.session.compose.as_ref().unwrap().id;
    h.run_steps(5);
    let at = h.state().hit.rect("composeAttachment", attachment_key(compose, 0)).unwrap().center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(30);
    assert!(h.state().hit.rect("attachmentPreviewText", compose).is_some() && h.state().hit.rect("attachmentPreviewBack", compose).is_some());
}

/// A file of a message being written is previewed with a click on its chip (v0.12.14).
#[test]
fn a_file_being_written_is_previewed_from_its_chip() {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.execute("view.previewPane", json!({"value": true})).unwrap();
    app.execute("mail.new", json!({"subject": "Notes"})).unwrap();
    app.execute("compose.attach", json!({"name": "notes.txt", "data": "aGVsbG8gd29ybGQ="})).unwrap();
    let compose = app.session.compose.as_ref().unwrap().id;
    h.run_steps(5);
    let at = h.state().hit.rect("composeAttachment", attachment_key(compose, 0)).unwrap().center();
    click(&mut h, at, PointerButton::Primary, 1);
    h.run_steps(10);
    assert!(h.state().hit.rect("attachmentPreviewText", compose).is_some());
}

/// A file Microsoft Defender flagged is never previewed, from its chip, its menu or a script,
/// nor that file forwarded in a message being written (v0.12.14).
#[test]
fn a_flagged_file_is_never_previewed() {
    use crate::attachment_scan::{ScanOutcome, ScanRequest, notice, tick};
    let dir = tempfile::tempdir().unwrap();
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    app.session.path = Some(dir.path().join("mailbox.mailcraft"));
    app.session.mailbox.message_mut(A).unwrap().attachments = vec![Attachment::new("notes.txt", "text/plain", b"looks harmless".to_vec())];
    app.services.scan_file = Some(Box::new(|_: ScanRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(ScanOutcome::Flagged { threats: vec!["Trojan:Test".to_string()] }).unwrap();
        rx
    }));
    notice(app, A);
    for _ in 0..16 {
        tick(app);
    }
    assert!(crate::attachment_scan::flagged(app, A, 0).is_some());
    app.execute("view.previewPane", json!({"value": true})).unwrap();
    app.run("mail.select", json!({"id": A})).unwrap();
    assert!(!single_click_allowed(app, Origin::Message(A), 0));
    assert!(!available(app, A, 0));
    assert!(app.execute("ui.previewAttachment", json!({"id": A, "index": 0})).is_err());
    assert!(app.attachment_preview.is_none());
    app.execute("mail.forward", json!({"id": A})).unwrap();
    let compose = app.session.compose.as_ref().unwrap().id;
    if !app.session.compose.as_ref().unwrap().attachments.is_empty() {
        assert!(!single_click_allowed(app, Origin::Draft(compose), 0));
        assert!(open_in(app, egui::ViewportId::ROOT, Origin::Draft(compose), 0).is_err());
    }
}

/// The question about sending says how many files go and shows each by its name, over the
/// message (v0.12.14).
#[test]
fn the_send_question_shows_each_file() {
    use crate::send_preview::{File, SendPreview};
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_model::Mailbox::default(), testing::now(), 0), crate::Services::default());
    app.autosave = false;
    let preview = SendPreview {
        message: Arc::new(mailcraft_model::Message { subject: "Invoice".into(), ..Default::default() }),
        files: vec![
            File { name: "invoice.pdf".into(), mime: "application/pdf".into(), size: 12_000 },
            File { name: "id_ed25519".into(), mime: "application/octet-stream".into(), size: 3 },
        ],
    };
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::send_preview::show(&mut app, ui, &preview, 300.0));
        out.textures_delta.clear();
    }
    assert!(app.hit.rect("sendPreviewAttachments", 0).is_some());
    assert!(app.hit.rect("sendPreviewAttachment", 0).is_some());
    assert!(app.hit.rect("sendPreviewAttachment", 1).is_some());
}
