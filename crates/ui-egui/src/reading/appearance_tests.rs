// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit reader/unsent draft presentation without modifying mail or app theme.

use mailcraft_engine::Session;
use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};
use serde_json::json;

use super::{MessageTheme, Place, appearance, view};
use crate::{MailApp, Services, UiState, theme::Appearance};

fn app() -> (MailApp, u64) {
    let mut mailbox = Mailbox::with_local_account("Owner", "owner@example.org");
    let id = mailbox
        .add_message(Message {
            folder: mailbox.inbox().unwrap(),
            subject: "Appearance fixture".into(),
            from: Address::new("Sender", "sender@example.org"),
            read: true,
            body: Body {
                text: "Unchanged body".into(),
                html: Some("<style>@media(prefers-color-scheme:dark){p{color:#abcdef}}</style><p>Unchanged body</p>".into()),
            },
            ..Default::default()
        })
        .unwrap();
    let mut app = MailApp::new(Session::new(mailbox), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app.execute("mail.select", json!({"id":id})).unwrap();
    (app, id)
}

#[test]
fn reader_override_works_in_both_directions_and_preserves_mail_and_app_theme() {
    let (app, id) = app();
    let mut h = super::tests::window(app, (1200.0, 850.0));
    let original = h.state().session.mailbox.message(id).unwrap().clone();
    for (app_theme, message_theme, dark) in [("dark", "light", false), ("light", "dark", true), ("dark", "app", true)] {
        h.state_mut().execute("ui.theme", json!({"value":app_theme})).unwrap();
        h.state_mut().execute("ui.messageTheme", json!({"value":message_theme})).unwrap();
        h.run_steps(8);
        assert_eq!(h.ctx.global_style().visuals.dark_mode, app_theme == "dark");
        assert_eq!(appearance::dark(h.state(), &h.ctx, Place::Pane), dark);
        assert_eq!(view::current_look(&h.ctx, Place::Pane, id).map(|look| look.dark()), Some(dark));
        assert_eq!(h.state().session.mailbox.message(id).unwrap(), &original);
    }
}

#[test]
fn display_choices_round_trip_and_legacy_light_preference_remains_effective() {
    let (mut app, _) = app();
    app.ui.theme = Appearance::Dark;
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, Appearance::Dark.preference(None), &crate::theme::ThemeChoice { appearance: Appearance::Dark, ..Default::default() });
    app.execute("ui.lightMessages", json!({"value":true})).unwrap();
    assert!(!appearance::dark(&app, &ctx, Place::Pane));
    app.execute("ui.messageTheme", json!({"value":"dark"})).unwrap();
    app.execute("ui.messageTheme", json!({"target":"draft","value":"light"})).unwrap();
    app.ui.preview_compose = Some((app.session.openings(), 8));
    let restored: UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
    assert_eq!(restored.message_theme, Some(MessageTheme::Dark));
    assert_eq!(restored.draft_preview_theme, MessageTheme::Light);
    assert_eq!(restored.preview_compose, None, "preview does not reopen an obsolete composer");
    let before = serde_json::to_value(&app.ui).unwrap();
    for params in [
        json!({"value":"sepia"}),
        json!({"target":false,"value":"dark"}),
        json!({"target":"other","value":"light"}),
        json!({"value":"dark","extra":1}),
    ] {
        assert!(app.execute("ui.messageTheme", params).is_err());
        assert_eq!(serde_json::to_value(&app.ui).unwrap(), before);
    }
    app.execute("ui.lightMessages", json!({"value":false})).unwrap();
    assert_eq!(app.ui.message_theme, None);
}

#[test]
fn authenticated_agent_can_switch_presentation_and_preview_without_an_owner_prompt() {
    let (mut app, _) = app();
    app.execute("mail.new", json!({"to":"recipient@example.org"})).unwrap();
    let original = app.session.compose.clone();
    let ctx = egui::Context::default();
    for (command, params) in [
        ("ui.messageTheme", json!({"target":"reader","value":"light"})),
        ("ui.messageTheme", json!({"target":"draft","value":"dark"})),
        ("ui.composePreview", json!({"on":true})),
        ("ui.composePreview", json!({"on":false})),
    ] {
        let (request, _) = crate::control::ControlRequest::new("engine.execute", json!({"command":command,"params":params}));
        let request = request.from_agent(Vec::new());
        match crate::control::handle(&mut app, &ctx, &request) {
            crate::control::Outcome::Done(answer) => assert_eq!(answer["ok"], true, "{answer}"),
            _ => panic!("display-only request must not ask for ownership or capture"),
        }
        assert_eq!(app.session.compose, original);
    }
}

#[test]
fn unsaved_preview_uses_the_real_builder_without_saving_sending_or_changing_draft() {
    let (mut app, _) = app();
    app.execute("mail.new", json!({"to":"not finished"})).unwrap();
    app.execute("compose.set", json!({"subject":"Preview only","body":"Unsent preview text"})).unwrap();
    let original = app.session.compose.clone();
    let revision = app.session.rev();
    let messages = app.session.mailbox.messages.len();
    let offline = app.fetching.offline;
    let preview = mailcraft_engine::cmd::compose::preview(&app.session).unwrap();
    assert_eq!(preview.subject, "Preview only");
    assert!(preview.body.text.contains("Unsent preview text"));
    assert_eq!(app.session.compose, original);
    assert_eq!((app.session.rev(), app.session.mailbox.messages.len()), (revision, messages));
    let compose = app.session.compose.as_ref().unwrap().id;
    app.execute("ui.composePreview", json!({"on":true})).unwrap();
    let mut h = super::tests::window(app, (900.0, 700.0));
    for (value, dark) in [("light", false), ("dark", true)] {
        h.state_mut().execute("ui.messageTheme", json!({"target":"draft","value":value})).unwrap();
        h.run_steps(8);
        // Plain text has no responsive HTML look to replace. Inspect the
        // actual native text jobs, rather than that parse-cache diagnostic.
        let expected = crate::theme::Tokens::message(&h.ctx, dark);
        let colours = view::kept_at(&h.ctx, Place::DraftPreview(compose), 0, |laid, _| {
            laid.items
                .iter()
                .filter_map(|item| match &item.kind {
                    super::layout::ItemKind::Text(text) if text.job.text.contains("Unsent preview text") => {
                        Some((text.paper, text.job.sections.iter().map(|section| section.format.color).collect::<Vec<_>>()))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .unwrap();
        assert!(!colours.is_empty(), "the unsaved text is rendered");
        for (paper, ink) in colours {
            assert_eq!(paper, expected.surface);
            assert!(!ink.is_empty());
            assert!(ink.iter().all(|colour| *colour == expected.text), "actual preview ink uses the selected palette");
        }
        assert_eq!(h.state().session.compose, original);
        assert_eq!(h.state().session.mailbox.messages.len(), messages);
        assert_eq!(h.state().fetching.offline, offline, "preview does not change connectivity");
    }
    h.state_mut().execute("ui.composePreview", json!({"on":false})).unwrap();
    h.run_steps(3);
    assert_eq!(h.state().session.compose, original);
    assert_eq!(h.state().ui.preview_compose, None);
}

#[test]
fn preview_builder_keeps_html_quotes_and_inline_picture_identity() {
    let (mut app, id) = app();
    app.execute("mail.reply", json!({"id":id})).unwrap();
    app.execute("compose.set", json!({"body":"My response"})).unwrap();
    let mut attachment = Attachment::new("logo.png", "image/png", super::tests::png(4, 4));
    attachment.inline = true;
    attachment.content_id = "logo@example.org".into();
    app.session.compose.as_mut().unwrap().attachments.push(attachment);
    let before = app.session.compose.clone();
    let first = mailcraft_engine::cmd::compose::preview(&app.session).unwrap();
    let second = mailcraft_engine::cmd::compose::preview(&app.session).unwrap();
    assert_eq!(first.body, second.body);
    assert_eq!(first.attachments, second.attachments);
    assert_eq!(first.attachments[0].content_id, "logo@example.org");
    assert!(first.body.html.as_deref().unwrap().contains("Unchanged body"));
    assert!(first.body.text.contains("My response"));
    assert_eq!(app.session.compose, before);
}

#[test]
fn a_separate_compose_layout_offers_preview_and_returns_to_the_same_unsaved_draft() {
    use egui_kittest::kittest::Queryable;
    let (mut app, _) = app();
    app.execute("mail.new", json!({"to":"recipient@example.org"})).unwrap();
    app.execute("compose.set", json!({"subject":"Separate composer","body":"Still unsent"})).unwrap();
    let original = app.session.compose.clone();
    let mut h =
        egui_kittest::Harness::builder().with_size(egui::vec2(800.0, 600.0)).with_accessibility_check(false).allow_missing_glyphs().build_ui_state(
            |ui, app: &mut MailApp| {
                crate::compose::show_in(app, ui, Some(1), false);
            },
            app,
        );
    h.run_steps(3);
    h.get_by_label("Preview message").click();
    h.run_steps(8);
    assert!(h.query_by_label("Read-only preview").is_some());
    assert!(h.query_by_label("Light").is_some() && h.query_by_label("Dark").is_some());
    assert_eq!(h.state().session.compose, original);
    h.get_by_label("Back to editing").click();
    h.run_steps(3);
    assert!(h.query_by_label("Preview message").is_some());
    assert_eq!(h.state().session.compose, original);
}

#[test]
#[ignore = "retained native appearance evidence; requires MAILCRAFT_APPEARANCE_SHOTS_DIR"]
fn shots_of_reader_and_unsent_draft_with_independent_light_dark_choices() {
    use egui_kittest::kittest::Queryable;
    let directory = std::path::PathBuf::from(std::env::var("MAILCRAFT_APPEARANCE_SHOTS_DIR").unwrap());
    assert!(directory.is_dir(), "the owner creates the exact retained evidence directory");
    for (app_theme, message_theme) in [("dark", "light"), ("light", "dark")] {
        for (width, height, dpi) in [(1440.0, 900.0, 100), (760.0, 480.0, 150)] {
            for draft in [false, true] {
                let (mut app, id) = app();
                app.execute("ui.theme", json!({"value":app_theme})).unwrap();
                if draft {
                    app.execute("mail.reply", json!({"id":id})).unwrap();
                    app.execute(
                        "compose.set",
                        json!({"subject":"Draft appearance preview","body":"Check the complete unsent message before sending."}),
                    )
                    .unwrap();
                    app.execute("ui.composePreview", json!({"on":true})).unwrap();
                }
                app.execute("ui.messageTheme", json!({"target":if draft {"draft"} else {"reader"},"value":message_theme})).unwrap();
                let mut h = egui_kittest::Harness::builder()
                    .with_size(egui::vec2(width, height))
                    .with_pixels_per_point(dpi as f32 / 100.0)
                    .with_accessibility_check(false)
                    .allow_missing_glyphs()
                    .wgpu()
                    .build_ui_state(
                        |ui, app: &mut MailApp| {
                            let ctx = ui.ctx().clone();
                            app.logic(&ctx);
                            app.ui(ui);
                        },
                        app,
                    );
                h.input_mut().max_texture_side = Some(8192);
                h.run_steps(8);
                let file = directory.join(format!(
                    "{}-app-{app_theme}-message-{message_theme}-{}-dpi{dpi}.png",
                    if draft { "draft" } else { "reader" },
                    width as u32
                ));
                assert!(!file.exists(), "preserve original rendered evidence");
                h.render().unwrap().save(&file).unwrap();
                let viewport = h.ctx.viewport_rect();
                for label in ["Light", "Dark", "App theme"] {
                    let rect = h.get_by_label(label).rect();
                    assert!(viewport.contains_rect(rect), "{label} remains accessible: {rect:?} inside {viewport:?}, {file:?}");
                }
                if draft {
                    assert!(viewport.contains_rect(h.get_by_label("Back to editing").rect()));
                }
                assert!(h.state().session.view.folder_pane, "message display preserves the owner's folder-pane preference");
                // Panels snap their edges to physical pixels. At fractional DPI
                // the persisted logical width may differ by that one pixel.
                let pixel = 1.0 / h.ctx.pixels_per_point();
                assert!((h.state().ui.folder_pane_width - 230.0).abs() <= pixel, "folder width must remain within one physical pixel");
                assert!(h.state().ui.list_width >= 240.0, "appearance controls must not squeeze message rows below their usable width");
            }
        }
    }
}
