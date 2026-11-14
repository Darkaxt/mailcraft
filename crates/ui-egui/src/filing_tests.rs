// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_engine::Session;
use mailcraft_model::{AccountKind, Address, Body, FolderKind, Message};

fn app() -> (MailApp, Id, Id, Id) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.account_mut(account).unwrap().kind = AccountKind::Microsoft;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let destination = s.mailbox.add_folder(account, Some(inbox), "Company").unwrap();
    let old = s.mailbox.add_folder(account, Some(inbox), "Old company").unwrap();
    let id = s
        .mailbox
        .add_message(Message {
            account,
            folder: inbox,
            from: Address::new("Company", "billing@example.org"),
            subject: "Invoice".into(),
            body: Body { text: "Complete local fixture".into(), html: None },
            ..Default::default()
        })
        .unwrap();
    s.run(
        "rules.add",
        &json!({"name":"Existing company","server":false,"conditions":[{"type":"fromAddresses","value":["billing@example.org"]}],
        "actions":[{"type":"moveTo","value":old}]}),
    )
    .unwrap();
    let mut app = MailApp::new(s, crate::Services::default());
    app.autosave = false;
    (app, account, id, destination)
}

#[test]
fn drop_opens_a_read_only_conflict_review_and_cancellation_changes_nothing() {
    let (mut app, account, id, destination) = app();
    app.execute("view.filing", json!({"account":account})).unwrap();
    let before = app.session.mailbox.clone();
    begin(&mut app, vec![id], destination);
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("filing"));
    assert_eq!(app.session.mailbox, before);
    app.execute("ui.closeDialog", json!({})).unwrap();
    assert_eq!(app.session.mailbox, before);
}

#[test]
fn real_tree_navigation_and_scrolling_review_use_existing_theme_and_keep_actions_reachable() {
    for theme in [crate::theme::Appearance::Light, crate::theme::Appearance::Dark] {
        let (mut app, account, id, destination) = app();
        app.ui.theme = theme;
        app.execute("view.filing", json!({"account":account})).unwrap();
        begin(&mut app, vec![id], destination);
        let before = app.session.mailbox.clone();
        let mut h = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        h.run_steps(4);
        assert!(h.state().hit.rect("unifiedFiling", 0).is_some());
        // Try Again shows only after something went wrong.
        assert!(h.state().hit.rect("filingRefresh", 0).is_none());
        for key in ["filingApply", "filingCancel"] {
            let rect = h.state().hit.rect(key, 0).unwrap();
            assert!(rect.top() >= 0.0 && rect.bottom() <= 700.0, "{key} remains reachable below scrolling content");
        }
        assert_eq!(h.state().session.mailbox, before, "rendering a review cannot file or edit rules");
        assert!(h.state().title_stem().starts_with(tl!("Unified filing")));
    }
}
