// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native, in-place source controls and their private export lifetime.

use std::sync::{Arc, Mutex, mpsc};

use egui::{Key, vec2};
use egui_kittest::{Harness, kittest::Queryable as _};
use mailcraft_engine::Session;
use mailcraft_model::{Address, Body, FolderKind, Message};
use serde_json::json;

use crate::{MailApp, Services, files::FileDialogRequest};

fn fixture(large: bool) -> (MailApp, u64) {
    let mut session = Session::empty();
    let account = session.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    let folder = session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let paragraph = "<p>Receipt from Example Studio: <b>café</b>. This is selectable source.</p>";
    let html = format!(
        "<html><body>{}<script>do_not_execute()</script><img src=\"https://tracker.example/pixel\"></body></html>",
        paragraph.repeat(if large { 1500 } else { 1 })
    );
    let id = session
        .mailbox
        .add_message(Message {
            account,
            folder,
            from: Address::new("Example Studio", "orders@example.org"),
            to: vec![Address::new("Owner", "owner@example.org")],
            subject: "Your studio receipt".into(),
            body: Body { text: "Your studio receipt is available offline.".into(), html: Some(html) },
            ..Default::default()
        })
        .unwrap();
    session.mailbox_changed();
    session.run("folder.select", &json!({"id":folder})).unwrap();
    session.prefs.mark_read_on_open = false;
    session.run("mail.select", &json!({"id":id})).unwrap();
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    (app, id)
}

fn settle(app: &mut MailApp) {
    crate::source_pane::tick(app);
    app.session.wait_source_exports();
    crate::source_pane::tick(app);
}

fn harness(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).with_size(vec2(1440.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let context = ui.ctx().clone();
            app.logic(&context);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(3);
    harness
}

#[test]
fn native_source_toggle_keyboard_selection_and_paging_preserve_the_message() {
    let (mut app, id) = fixture(true);
    let original = serde_json::to_value(&app.session.mailbox).unwrap();
    app.execute("ui.source", json!({"on":true})).unwrap();
    settle(&mut app);
    let mut h = harness(app);
    assert_eq!(h.state().source_pane.inspect()["format"], "html");
    assert!(h.query_by_label("HTML source").is_some());
    h.get_by_label("Selectable message source").focus();
    h.run_steps(1);
    h.key_press(Key::A);
    h.run_steps(1);
    assert_eq!(serde_json::to_value(&h.state().session.mailbox).unwrap(), original);
    h.get_by_label("Next page").click();
    h.run_steps(2);
    assert!(h.state().source_pane.inspect()["offset"].as_u64().unwrap() > 0);
    h.get_by_label("Previous page").click();
    h.run_steps(2);
    assert_eq!(h.state().source_pane.inspect()["offset"], 0);
    h.get_by_label("Message").click();
    h.run_steps(3);
    assert!(!h.state().source_pane.showing(id));
    assert!(h.query_by_label("HTML source").is_none());
    h.get_by_label("Source").focus();
    h.run_steps(1);
    h.key_press(Key::Space);
    h.run_steps(1);
    settle(h.state_mut());
    h.run_steps(2);
    assert!(h.state().source_pane.showing(id));
    assert!(h.state().dialog.is_none());
    assert_eq!(serde_json::to_value(&h.state().session.mailbox).unwrap(), original);
}

#[test]
fn source_branch_does_not_request_remote_images_even_when_rendered_mail_would() {
    let (mut app, id) = fixture(false);
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = asked.clone();
    app.services.pictures = Some(Box::new(move |request| {
        seen.lock().unwrap().push(format!("{request:?}"));
        mpsc::channel().1
    }));
    let seen = asked.clone();
    app.services.sender_images = Some(Box::new(move |address, _| {
        seen.lock().unwrap().push(address);
        mpsc::channel().1
    }));
    app.ui.download_pictures = true;
    app.ui.sender_images = true;
    app.execute("ui.source", json!({"on":true})).unwrap();
    settle(&mut app);
    let message = app.session.mailbox.messages.get(&id).unwrap().clone();
    let context = egui::Context::default();
    context
        .run_ui(egui::RawInput::default(), |ui| crate::reading::show_message(&mut app, ui, &message, crate::reading::Place::Pane))
        .drop_without_applying_deltas();
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(app.source_pane.inspect()["state"], "ready");
}

#[test]
fn cancelling_native_export_picker_releases_its_prepared_token() {
    let (mut app, _) = fixture(false);
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    app.services.file_dialog = Some(Box::new(move |request| {
        seen.lock().unwrap().push(request);
        let (sender, receiver) = mpsc::channel();
        sender.send(None).unwrap();
        receiver
    }));
    app.execute("ui.source", json!({"on":true})).unwrap();
    settle(&mut app);
    app.execute("ui.source", json!({"export":true})).unwrap();
    settle(&mut app);
    assert!(matches!(asked.lock().unwrap().as_slice(),[FileDialogRequest::Save{name}] if name.ends_with(".eml")));
    assert!(app.file_dialog.is_none());
    assert!(!app.session.source_export_busy());
    // Only the body token remains. Three simultaneously owned EML readers must
    // still fit; a leaked picker token would exhaust the four-reader pool.
    let id = app.session.sel.focus().unwrap();
    let mut tokens = Vec::new();
    for _ in 0..3 {
        let token = app.execute("mail.exportEml", json!({"id":id})).unwrap()["token"].as_u64().unwrap();
        app.session.wait_source_exports();
        tokens.push(token);
    }
    for token in tokens {
        app.execute("mail.exportEml", json!({"id":id,"token":token,"close":true})).unwrap();
    }
}

#[test]
#[ignore = "retained offscreen native source evidence; requires MAILCRAFT_SOURCE_SHOTS_DIR"]
fn shots_of_source_reader_in_both_themes_and_native_viewports() {
    let directory = std::env::var("MAILCRAFT_SOURCE_SHOTS_DIR").unwrap();
    for theme in ["light", "dark"] {
        for (width, height, pixels) in [(1440.0, 900.0, 1.0), (1920.0, 1080.0, 1.25), (960.0, 720.0, 1.0), (960.0, 720.0, 1.5)] {
            let (mut app, id) = fixture(false);
            app.execute("ui.theme", json!({"value":theme})).unwrap();
            app.execute("ui.source", json!({"on":true})).unwrap();
            settle(&mut app);
            let mut h = Harness::builder()
                .with_size(vec2(width, height))
                .with_pixels_per_point(pixels)
                .with_accessibility_check(false)
                .wgpu()
                .build_ui_state(
                    |ui, app: &mut MailApp| {
                        let context = ui.ctx().clone();
                        app.logic(&context);
                        app.ui(ui);
                    },
                    app,
                );
            h.input_mut().max_texture_side = Some(8192);
            h.run_steps(4);
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, height));
            let file = std::path::Path::new(&directory).join(format!("source-{theme}-{}-{}.png", width as u32, (pixels * 100.0) as u32));
            h.render().unwrap().save(&file).unwrap();
            for label in ["Source", "Message", "HTML source", "Export .eml"] {
                assert!(viewport.contains_rect(h.get_by_label(label).rect()), "{label} at {width}x{height}@{pixels}");
            }
            let body = h.state().hit.rect("sourceBody", id).unwrap();
            assert!(body.intersects(viewport));
            // The editor's unwrapped content may extend beyond the viewport;
            // the visible label and scroll viewport must remain on screen.
            // Accessibility queries by the label name find the editor itself.
            assert!(viewport.contains_rect(h.state().hit.rect("sourceLabel", id).unwrap()));
            let code_view = h.state().hit.rect("sourceViewport", id).unwrap();
            assert!(viewport.contains_rect(code_view));
            assert!(code_view.height() >= 100.0, "source remains readable: {code_view:?}");
            if width == 960.0 {
                assert!(body.width() > code_view.width(), "long source lines scroll horizontally");
            }
            println!("{}", file.display());
        }
    }
}

#[test]
fn prepared_source_export_uses_its_complete_snapshot_without_another_download() {
    let (mut app, id) = fixture(false);
    let token = app.execute("mail.exportEml", json!({"id":id})).unwrap()["token"].as_u64().unwrap();
    app.session.wait_source_exports();
    // Retention or sync can change the live record after preparation. The
    // already verified snapshot remains complete and needs no network work.
    let message = Arc::make_mut(app.session.mailbox.messages.get_mut(&id).unwrap());
    message.unfetched = true;
    message.server =
        Some(mailcraft_model::sync::ServerMessage { id: "synthetic-server-message".into(), folder: "inbox".into(), ..Default::default() });
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    app.services.file_dialog = Some(Box::new(move |request| {
        seen.lock().unwrap().push(request);
        let (sender, receiver) = mpsc::channel();
        sender.send(None).unwrap();
        receiver
    }));
    assert_eq!(app.run("file.exportMessage", json!({"id":id,"token":token})).unwrap()["cancelled"], true);
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(!app.fetching.busy());
    assert!(app.execute("mail.exportEml", json!({"id":id,"token":token})).is_err());
}
