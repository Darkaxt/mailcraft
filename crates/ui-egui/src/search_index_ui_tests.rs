// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Product controls, mailbox preservation and actual offscreen design evidence.

use egui::{Key, vec2};
use egui_kittest::{Harness, kittest::Queryable as _};
use mailcraft_engine::Session;
use mailcraft_model::{Account, Address, Attachment, Body, FolderKind, Id, Message};
use serde_json::json;

use crate::{MailApp, Services};

fn app() -> MailApp {
    let session = Session::new(mailcraft_engine::sample::mailbox(1_791_993_600, 0));
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all":true}));
    app
}

fn indexed_app() -> (MailApp, tempfile::TempDir, Id) {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("coverage.mailcraft");
    let mut mailbox = mailcraft_engine::sample::mailbox(1_791_993_600, 0);
    mailbox.add_account(Account { name: "Example work".into(), email: "work@example.org".into(), ..Default::default() });
    mailbox.add_account(Account { name: "Example archive".into(), email: "archive@example.org".into(), ..Default::default() });
    let accounts: Vec<_> = mailbox.accounts.iter().map(|account| account.id).collect();
    let mut selected = None;
    for account in accounts {
        let folder = mailbox.folders.iter().find(|folder| folder.account == account && folder.kind == FolderKind::Inbox).unwrap().id;
        let id = mailbox
            .add_message(Message {
                account,
                folder,
                from: Address::new("Inert coverage fixture", "receipt@example.net"),
                subject: "Café invoice".into(),
                body: Body { text: "Café invoice with retained searchable text.".into(), html: None },
                attachments: vec![Attachment::new("opaque.craftfixture", "application/x-craft-fixture", b"Explicitly unsupported fixture".to_vec())],
                ..Default::default()
            })
            .unwrap();
        selected.get_or_insert(id);
    }
    mailcraft_engine::store::save(&path, &mailbox).unwrap();
    let mut session = Session::new(mailbox);
    session.path = Some(path);
    session.index_words = true;
    session.prefs.offline = true;
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app.execute("ui.dismissReminder", json!({"all":true})).unwrap();
    (app, scratch, selected.unwrap())
}

fn wait_index(harness: &mut Harness<'_, MailApp>) {
    loop {
        harness.run_steps(1);
        let status = harness.state().session.content_index_status();
        assert!(status["problem"].is_null(), "{status}");
        if status["initialized"] == true
            && status["building"] == false
            && status["coverageCurrent"] == true
            && status["metadataPending"] == 0
            && status["bodiesPending"] == 0
            && status["attachmentsPending"] == 0
        {
            assert_eq!(status["coverage"]["accounts"].as_array().unwrap().len(), 3);
            return;
        }
    }
}

#[test]
fn coverage_reads_real_counts_and_source_pages_without_changing_the_search_or_selection() {
    let (mut app, _scratch, id) = indexed_app();
    app.execute("view.search", json!({"query":"\"Café invoice\""})).unwrap();
    let query = app.session.view.search.clone();
    app.execute("ui.searchIndex", json!({"view":"overview"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).with_size(vec2(900.0, 700.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    wait_index(&mut h);
    // The search's answer lists the message before it is selected: selecting what the list does
    // not show yet would show its whole folder, with no search (#485).
    let started = std::time::Instant::now();
    while !h.state_mut().session.rows().iter().any(|row| row.members.contains(&id)) {
        assert!(started.elapsed().as_secs() < 120, "the search never listed the message");
        h.run_steps(1);
    }
    h.state_mut().execute("mail.select", json!({"id":id})).unwrap();
    wait_index(&mut h);
    let selected = vec![id];
    for (view, field) in [("gaps", "gaps"), ("sources", "sources")] {
        h.state_mut().execute("ui.searchIndex", json!({"view":view,"id":id})).unwrap();
        loop {
            h.run_steps(1);
            let pane = h.state().search_index_pane.inspect();
            assert!(pane["error"].is_null(), "{pane}");
            if pane["page"]["state"] == "ready" {
                assert!(!pane["page"]["result"][field].as_array().unwrap().is_empty(), "{pane}");
                break;
            }
            assert_eq!(pane["page"]["state"], "pending", "{pane}");
        }
    }
    assert_eq!(h.state().session.sel.messages, selected);
    assert_eq!(h.state().session.view.search, query);
    h.state_mut().execute("ui.searchIndex", json!({"on":false})).unwrap();
    while h.state().search_index_pane.inspect()["retiring"].as_u64().unwrap() != 0 {
        h.run_steps(1);
    }
}

#[test]
fn case_control_preserves_literal_text_and_selection_and_coverage_escape_keeps_the_search() {
    let mut app = app();
    app.execute("view.search", json!({"query":"subject:\"A Café, precisely!\" from:receipts"})).unwrap();
    let original = serde_json::to_value(&app.session.mailbox).unwrap();
    let selection = app.session.sel.clone();
    app.execute("ui.searchCase", json!({"sensitive":true})).unwrap();
    assert!(app.session.view.search.contains("subject:\"A Café, precisely!\""));
    assert!(app.session.view.search.contains("from:receipts"));
    assert!(mailcraft_model::search::Query::parse(&app.session.view.search).case_sensitive());
    app.execute("ui.searchIndex", json!({"view":"overview"})).unwrap();
    let search = app.session.view.search.clone();
    let mut h = Harness::builder().with_accessibility_check(false).with_size(vec2(1440.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    assert!(h.get_by_label("Case sensitive").rect().width() > 0.0);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().search_index_pane.is_open());
    assert_eq!(h.state().session.view.search, search);
    assert_eq!(h.state().session.sel.messages, selection.messages);
    assert_eq!(serde_json::to_value(&h.state().session.mailbox).unwrap(), original);
}

#[test]
fn coverage_rejects_unknown_views_and_does_not_change_the_open_mailbox() {
    let mut app = app();
    let original = serde_json::to_value(&app.session.mailbox).unwrap();
    assert!(app.execute("ui.searchIndex", json!({"view":"delete"})).is_err());
    assert!(app.execute("ui.searchCase", json!({"sensitive":"true"})).is_err());
    assert_eq!(serde_json::to_value(&app.session.mailbox).unwrap(), original);
}

#[test]
#[ignore = "retained offscreen index-stage evidence; requires MAILCRAFT_INDEX_SHOTS_DIR"]
fn shots_of_search_coverage_in_both_themes_and_compact_enlarged_windows() {
    let directory = std::env::var("MAILCRAFT_INDEX_SHOTS_DIR").unwrap();
    for theme in ["light", "dark"] {
        for (width, height, scale) in [(1440.0, 900.0, 1.0), (900.0, 700.0, 1.0), (900.0, 700.0, 1.5)] {
            let (mut app, _scratch, id) = indexed_app();
            app.execute("ui.theme", json!({"value":theme})).unwrap();
            app.execute("view.search", json!({"query":"\"Café invoice\""})).unwrap();
            app.execute("ui.searchIndex", json!({"view":"overview"})).unwrap();
            let mut h =
                Harness::builder().with_size(vec2(width, height)).with_pixels_per_point(scale).with_accessibility_check(false).wgpu().build_ui_state(
                    |ui, app: &mut MailApp| {
                        let ctx = ui.ctx().clone();
                        app.logic(&ctx);
                        app.ui(ui);
                    },
                    app,
                );
            h.input_mut().max_texture_side = Some(8192);
            wait_index(&mut h);
            h.state_mut().execute("mail.select", json!({"id":id})).unwrap();
            wait_index(&mut h);
            h.run_steps(2);
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, height));
            let panel = h.ctx.memory(|memory| memory.area_rect(egui::Id::unique("search_content_coverage"))).unwrap();
            assert!(viewport.contains_rect(panel), "panel {panel:?} at {width}x{height}@{scale}");
            assert!(panel.width() <= 760.0 && panel.height() <= 600.0, "panel {panel:?} exceeds its design contract");
            for label in ["Overview", "Missing content", "Case sensitive", "Retry unavailable content", "Close"] {
                assert!(viewport.contains_rect(h.get_by_label(label).rect()), "{label} at {width}x{height}@{scale}");
            }
            let output = std::path::Path::new(&directory).join(format!("search-coverage-{theme}-{}-{}.png", width as u32, (scale * 100.0) as u32));
            h.render().unwrap().save(output).unwrap();
        }
    }
}

#[test]
fn a_ready_inspection_is_polled_again_only_when_its_inputs_change() {
    let mut app = app();
    let now = crate::search_index_ui::observed(&app);
    // An unknown command fails when dispatched, so the page state shows whether tick polled.
    app.search_index_pane = crate::search_index_ui::Pane::ready_for_test("test.notACommand", app.session.openings(), now);
    for _ in 0..3 {
        crate::search_index_ui::tick(&mut app);
        assert_eq!(app.search_index_pane.page_state(), Some("ready"), "an unchanged ready page is not dispatched every frame");
    }
    let changed = (now.0.wrapping_add(1), now.1, now.2);
    app.search_index_pane = crate::search_index_ui::Pane::ready_for_test("test.notACommand", app.session.openings(), changed);
    crate::search_index_ui::tick(&mut app);
    assert_eq!(app.search_index_pane.page_state(), Some("failed"), "a changed revision re-polls the ready page");
}
