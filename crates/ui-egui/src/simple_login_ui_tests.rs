// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native virtual alias navigation and reported identity use the real app UI.

use egui::{Key, vec2};
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::{MailApp, Services};

fn app() -> (MailApp, Id, Id) {
    let mut session = Session::empty();
    let account = session.run("account.add", &json!({"email":"owner@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let mut first = 0;
    for alias in ["shop@aliases.example", "news@aliases.example", "a-long-company-receiving-alias@a-long-private-domain.example"] {
        let identity = session
            .mailbox
            .add_message(Message {
                folder: inbox,
                from: Address::new("Shop through SimpleLogin", "sender_random@simplelogin.co"),
                to: vec![Address::new("", alias)],
                subject: if first == 0 { "Your order receipt" } else { alias }.into(),
                message_id: format!("{}@fixture.example", alias.replace('@', ".")),
                date: 1_791_993_600,
                body: mailcraft_model::Body { text: "Order details remain available offline.".into(), html: None },
                headers: vec![
                    ("X-SimpleLogin-Type".into(), "Forward".into()),
                    ("X-SimpleLogin-Envelope-To".into(), alias.into()),
                    ("Received".into(), "from mail-200.simplelogin.co (mail-200.simplelogin.co [192.0.2.10]) by mx.example.org".into()),
                    ("X-SimpleLogin-Original-From".into(), "Shop Team <orders@shop.example>".into()),
                ],
                ..Default::default()
            })
            .unwrap();
        if first == 0 {
            first = identity;
        }
    }
    session.mailbox_changed();
    session.run("folder.select", &json!({"id":inbox})).unwrap();
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    app.session.prefs.mark_read_on_open = false;
    app.ui.folder_pane_width = 280.0;
    (app, account, first)
}

fn harness(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    let mut harness = egui_kittest::Harness::builder().with_size(vec2(1440.0, 900.0)).with_accessibility_check(false).build_ui_state(
        |ui, app: &mut MailApp| {
            let context = ui.ctx().clone();
            app.logic(&context);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

#[test]
fn simple_login_alias_navigation_supports_keyboard_and_preserves_physical_folders() {
    let (app, account, _) = app();
    let original = serde_json::to_value(&app.session.mailbox).unwrap();
    let mut harness = harness(app);
    let heading = harness.state().hit.rect("aliasHeading", account).unwrap();
    assert!(heading.height() >= 24.0);
    harness.get_by_label("SimpleLogin aliases").focus();
    harness.run_steps(2);
    harness.key_press(Key::Space);
    harness.run_steps(3);
    assert!(harness.state().ui.expanded_alias_accounts.contains(&account));
    harness.get_by_label("shop@aliases.example").click();
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.search, "alias:shop@aliases.example");
    assert_eq!(harness.state_mut().session.rows().len(), 1);
    assert_eq!(serde_json::to_value(&harness.state().session.mailbox).unwrap(), original);
    let settings = serde_json::to_vec(&harness.state().prefs()).unwrap();
    let reopened: crate::UiState = serde_json::from_slice(&settings).unwrap();
    assert_eq!(reopened.expanded_alias_accounts, [account]);
    assert!(reopened.view.search.is_empty(), "the existing private-search startup policy is preserved");
    harness.state_mut().execute("ui.toggleAliases", json!({"account":account})).unwrap();
    harness.run_steps(2);
    assert!(!harness.state().ui.expanded_alias_accounts.contains(&account));
}

#[test]
fn simple_login_reported_header_preserves_native_reply_identity() {
    let (mut app, _, message) = app();
    app.session.run("mail.select", &json!({"id":message})).unwrap();
    let mut harness = harness(app);
    assert!(harness.query_all_by_label("Shop Team").next().is_some());
    assert!(harness.query_all_by_label("<orders@shop.example>").next().is_some());
    assert!(harness.query_all_by_label("Reply through: Shop through SimpleLogin <sender_random@simplelogin.co>").next().is_some());
    assert_eq!(harness.state().session.mailbox.message(message).unwrap().from.email, "sender_random@simplelogin.co");
    harness.state_mut().run("mail.reply", json!({"id":message})).unwrap();
    assert_eq!(harness.state().session.compose.as_ref().unwrap().to, "Shop through SimpleLogin <sender_random@simplelogin.co>");
}

#[test]
#[ignore = "retained offscreen native evidence; requires MAILCRAFT_ALIAS_SHOTS_DIR"]
fn shots_of_simple_login_alias_filters_and_reading_identity() {
    let directory = std::env::var("MAILCRAFT_ALIAS_SHOTS_DIR").unwrap();
    for theme in ["light", "dark"] {
        for (width, height, pixels) in [(1440.0, 900.0, 1.0), (1920.0, 1080.0, 1.25), (960.0, 720.0, 1.0), (960.0, 720.0, 1.5)] {
            let (mut app, account, message) = app();
            app.execute("ui.theme", json!({"value":theme})).unwrap();
            app.execute("ui.toggleAliases", json!({"account":account})).unwrap();
            app.run("view.alias", json!({"account":account,"alias":"shop@aliases.example"})).unwrap();
            app.run("mail.select", json!({"id":message})).unwrap();
            let mut harness = egui_kittest::Harness::builder()
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
            harness.input_mut().max_texture_side = Some(8192);
            harness.run_steps(4);
            let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, height));
            for label in ["SimpleLogin aliases", "shop@aliases.example", "Shop Team", "Reported via SimpleLogin: shop@aliases.example"] {
                let rect = harness.get_by_label(label).rect();
                assert!(viewport.contains_rect(rect), "{label} remains visible at {width}x{height}@{pixels}: {rect:?}");
            }
            let file = std::path::Path::new(&directory).join(format!("aliases-{theme}-{}-{}.png", width as u32, (pixels * 100.0) as u32));
            harness.render().unwrap().save(&file).unwrap();
            println!("{}", file.display());
        }
    }
}
