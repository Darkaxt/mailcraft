// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::people_view::testing;
use mailcraft_model::{Condition, RuleField, RuleText};

#[test]
fn simple_login_predicates_roundtrip_through_the_destination_editor_without_widening() {
    let (mut session, source, _, destination) = fixture();
    for field in ["originalFrom", "receivingAlias"] {
        let created = session
            .run(
                "rules.add",
                &json!({"name":"Identity filing","account":source,"server":false,"enabled":false,
            "conditions":[{"type":"text","value":{"field":field,"pattern":"^billing@example\\.org$","regex":true,"caseSensitive":true}}],
            "actions":[{"type":"moveTo","value":destination}]}),
            )
            .unwrap();
        let id = created["id"].as_u64().unwrap();
        let original = session.mailbox.rules.iter().find(|rule| rule.id == id).unwrap().clone();
        let mut form = RulesForm::new(&session, &json!({"destination":destination}));
        form.edit_rule(&session.mailbox, id);
        let (command, params) = form.command();
        assert_eq!(command, "rules.edit");
        assert_eq!(params["conditions"][0]["value"]["field"], field);
        session.run(command, &params).unwrap();
        assert_eq!(session.mailbox.rules.iter().find(|rule| rule.id == id).unwrap(), &original);
    }
}

fn fixture() -> (Session, Id, Id, Id) {
    let mut session = Session::empty();
    let source = session.run("account.add", &json!({"email": "source@example.org"})).unwrap()["id"].as_u64().unwrap();
    let target = session.run("account.add", &json!({"email": "target@example.org"})).unwrap()["id"].as_u64().unwrap();
    let from = session.mailbox.folder_of_kind(source, FolderKind::Inbox).unwrap();
    let to = session.mailbox.folder_of_kind(target, FolderKind::Inbox).unwrap();
    (session, source, from, to)
}

#[test]
fn exact_native_address_conditions_round_trip_through_the_folder_editor() {
    let (mut session, source, _, target) = fixture();
    let made = session
        .run(
            "rules.add",
            &json!({"name":"Exact correspondents","account":source,"server":false,
        "conditions":[{"type":"fromAddresses","value":["billing@example.org","no-reply=service@example.org"]},
            {"type":"toAddresses","value":["office@aliases.example"]}],
        "actions":[{"type":"moveTo","value":target}]}),
        )
        .unwrap();
    let id = made["id"].as_u64().unwrap();
    let original = session.mailbox.rules.iter().find(|rule| rule.id == id).unwrap().clone();
    let mut form = RulesForm::new(&session, &json!({"destination":target}));
    form.edit_rule(&session.mailbox, id);
    let (command, params) = form.command();
    assert_eq!(command, "rules.edit");
    assert_eq!(params["conditions"][0]["type"], "fromAddresses");
    assert_eq!(params["conditions"][1]["type"], "toAddresses");
    session.run(command, &params).unwrap();
    assert_eq!(session.mailbox.rules.iter().find(|rule| rule.id == id).unwrap().conditions, original.conditions);
}

#[test]
fn destination_form_includes_disabled_shared_rules_and_preserves_target_on_source_change() {
    let (mut session, source, from, to) = fixture();
    session.mailbox.rules = vec![
        Rule {
            id: 501,
            enabled: false,
            account: source,
            conditions: vec![Condition::HasAttachment],
            actions: vec![Action::MoveTo(to)],
            ..Default::default()
        },
        Rule {
            id: 502,
            account: source,
            conditions: vec![Condition::HasAttachment],
            actions: vec![Action::CopyTo(to), Action::MoveTo(from)],
            ..Default::default()
        },
        Rule { id: 503, account: source, conditions: vec![Condition::HasAttachment], actions: vec![Action::MoveTo(from)], ..Default::default() },
    ];
    let mut form = RulesForm::new(&session, &json!({"destination": to}));
    assert_eq!(form.listed_rules(&session.mailbox).iter().map(|r| r.id).collect::<Vec<_>>(), [501, 502]);
    form.choose(&session.mailbox, source);
    form.reset();
    assert_eq!(form.command().1["actions"][0]["value"], to);
    assert_eq!(form.command().1["account"], source);
    assert_eq!(form.command().1["server"], false);
    assert_ne!(from, to);
}

#[test]
fn editing_round_trips_conditions_exceptions_copy_actions_and_global_identity() {
    let (mut session, _, _, to) = fixture();
    let rule = Rule {
        id: 501,
        name: "Invoices".into(),
        enabled: false,
        conditions: vec![
            Condition::Text(RuleText { field: RuleField::Subject, pattern: "^Invoice [0-9]+$".into(), regex: true, case_sensitive: true }),
            Condition::Unless(vec![Condition::FromContains("archive@example.org".into())]),
        ],
        actions: vec![Action::CopyTo(to), Action::Flag],
        stop: true,
        ..Default::default()
    };
    session.mailbox.rules.push(rule.clone());
    let mut form = RulesForm::new(&session, &json!({"destination": to}));
    form.edit_rule(&session.mailbox, rule.id);
    assert!(form.ready());
    let (command, params) = form.command();
    assert_eq!(command, "rules.edit");
    assert_eq!(params["conditions"][0], serde_json::to_value(&rule.conditions[0]).unwrap());
    assert_eq!(params["exceptions"], json!([{"type": "fromContains", "value": "archive@example.org"}]));
    assert_eq!(params["actions"], serde_json::to_value(&rule.actions).unwrap());
    session.run(command, &params).unwrap();
    assert_eq!(session.mailbox.rules[0], rule);
}

fn rendered_app() -> (crate::MailApp, Id, Id) {
    let (mut session, source, from, to) = fixture();
    session.mailbox.add_message(mailcraft_model::Message { folder: from, subject: "Invoice 123".into(), ..Default::default() }).unwrap();
    session.mailbox.rules.push(Rule {
        id: 501,
        name: "File invoices".into(),
        enabled: false,
        account: source,
        conditions: vec![
            Condition::Text(RuleText { field: RuleField::Subject, pattern: "^Invoice [0-9]+$".into(), regex: true, case_sensitive: true }),
            Condition::Unless(vec![Condition::FromContains("archive@example.org".into())]),
        ],
        actions: vec![Action::MoveTo(to)],
        ..Default::default()
    });
    let mut app = crate::MailApp::new(session, crate::Services::default());
    app.autosave = false;
    (app, from, to)
}

#[test]
fn merged_sender_sources_round_trip_through_the_destination_editor_without_a_merge_request() {
    let (mut session, source, _, to) = fixture();
    let add = |sender: &str| {
        json!({"name": "Company", "account": source, "server": false,
        "conditions": [{"type":"fromContains", "value":sender}], "actions":[{"type":"moveTo", "value":to}]})
    };
    let id = session.run("rules.add", &add("billing@example.org")).unwrap()["id"].as_u64().unwrap();
    session.run("rules.add", &add("news@example.org")).unwrap();
    assert_eq!(session.mailbox.rules.len(), 1);
    let original = session.mailbox.rules[0].clone();
    let mut form = RulesForm::new(&session, &json!({"destination":to}));
    form.edit_rule(&session.mailbox, id);
    assert!(form.ready());
    let (command, params) = form.command();
    assert_eq!(params["conditions"][0], json!({"type":"fromAny", "value":["billing@example.org", "news@example.org"]}));
    session.run(command, &params).unwrap();
    assert_eq!(session.mailbox.rules, [original]);
}

fn window(app: crate::MailApp, size: egui::Vec2) -> egui_kittest::Harness<'static, crate::MailApp> {
    let mut harness = egui_kittest::Harness::builder().with_size(size).with_accessibility_check(false).build_ui_state(
        |ui, app: &mut crate::MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

fn conflicting_app() -> crate::MailApp {
    let (mut app, from, to) = rendered_app();
    app.session.mailbox.rules[0].enabled = true;
    app.session.mailbox.rules[0].conditions = vec![Condition::FromAny(vec!["billing@example.org".into(), "news@example.org".into()])];
    let mut hidden = app.session.mailbox.rules[0].clone();
    hidden.id = 502;
    hidden.name = "Company rule in another folder".into();
    hidden.actions = vec![Action::MoveTo(from)];
    // A pre-existing conflict must stay inspectable even though new activation is refused.
    app.session.mailbox.rules.push(hidden);
    app.execute("ui.dialog", json!({"name":"rules","destination":to})).unwrap();
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &mut app.dialog else { panic!("editor opens") };
    form.edit_rule(&app.session.mailbox, 501);
    app
}

#[test]
fn conflict_inspection_includes_other_destinations_and_save_preserves_conflicting_originals() {
    use egui_kittest::kittest::Queryable as _;
    let app = conflicting_app();
    let original = app.session.mailbox.rules.clone();
    let size = vec2(760.0, 480.0);
    let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    let mut harness = window(app, size);
    assert!(viewport.contains_rect(harness.get_by_label("Check conflicts").rect()));
    harness.get_by_label("Check conflicts").click();
    harness.run_steps(3);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else { panic!("editor stays open") };
    assert_eq!(form.preview.as_ref().unwrap()["confirmed"], 1);
    assert_eq!(form.preview.as_ref().unwrap()["conflicts"][0]["id"], 502);
    assert_eq!(form.listed_rules(&harness.state().session.mailbox).len(), 1, "analysis does not broaden the folder's rule list");
    assert_eq!(harness.state().session.mailbox.rules, original);
    harness.get_by_label("Save Rule").click();
    harness.run_steps(3);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else {
        panic!("failed save keeps editor open")
    };
    assert!(form.error.contains("filing conflict"));
    assert_eq!(harness.state().session.mailbox.rules, original);
    for label in ["Save Rule", "Preview", "Check conflicts", "Cancel edit", "Close"] {
        assert!(viewport.contains_rect(harness.get_by_label(label).rect()), "{label} stays visible");
    }
}

#[test]
#[ignore = "retained synthetic render evidence; requires MAILCRAFT_RULE_SHOTS_DIR"]
fn shots_of_destination_rule_editor_conflicts() {
    use egui_kittest::kittest::Queryable as _;
    let directory = std::env::var("MAILCRAFT_RULE_SHOTS_DIR").unwrap();
    for theme in ["light", "dark"] {
        for size in [vec2(760.0, 480.0), vec2(1440.0, 900.0)] {
            for pixels in [1.0, 1.5] {
                let mut app = conflicting_app();
                app.execute("ui.theme", json!({"value":theme})).unwrap();
                let mut harness = egui_kittest::Harness::builder()
                    .with_size(size)
                    .with_pixels_per_point(pixels)
                    .with_accessibility_check(false)
                    .wgpu()
                    .build_ui_state(
                        |ui, app: &mut crate::MailApp| {
                            let ctx = ui.ctx().clone();
                            app.logic(&ctx);
                            app.ui(ui);
                        },
                        app,
                    );
                harness.input_mut().max_texture_side = Some(8192);
                harness.run_steps(4);
                harness.get_by_label("Check conflicts").click();
                harness.run_steps(3);
                harness.get_by_label("Conflicting destinations: 1 confirmed; 0 potential.").scroll_to_me();
                harness.run_steps(4);
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                for label in ["Save Rule", "Preview", "Check conflicts", "Cancel edit", "Close"] {
                    assert!(viewport.contains_rect(harness.get_by_label(label).rect()));
                }
                let path =
                    std::path::Path::new(&directory).join(format!("rules-{theme}-{}-{}-conflicts.png", size.x as u32, (pixels * 100.0) as u32));
                harness.render().unwrap().save(&path).unwrap();
                println!("{}", path.display());
            }
        }
    }
}

#[test]
fn cog_opens_exact_destination_without_selecting_it_and_editor_preserves_regex() {
    use egui_kittest::kittest::Queryable as _;
    let (app, from, to) = rendered_app();
    let mut harness = window(app, vec2(1280.0, 900.0));
    assert_eq!(harness.state().session.view.folder, from);
    let cog = harness.state().hit.rect("folderRules", to).unwrap().center();
    testing::click(&mut harness, cog, egui::PointerButton::Primary, 1);
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.folder, from);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else {
        panic!("the scoped rule editor must open")
    };
    assert_eq!(form.destination, Some(to));
    assert_eq!(form.listed_rules(&harness.state().session.mailbox).len(), 1);
    harness.get_by_label("Edit this local rule").click();
    harness.run_steps(3);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else { panic!("the rule editor stays open") };
    assert_eq!(form.editing, Some(501));
    assert!(form.conds[0].regex && form.conds[0].case_sensitive);
    assert!(form.conds[1].exception);
    harness.get_by_label("Save Rule").click();
    harness.run_steps(3);
    assert_eq!(harness.state().session.mailbox.rules[0].id, 501);
    assert!(!harness.state().session.mailbox.rules[0].enabled);
}

#[test]
fn folder_rules_open_from_the_context_menu_and_from_a_keyboard_cog() {
    use egui_kittest::kittest::Queryable as _;
    let (app, from, to) = rendered_app();
    let mut harness = window(app, vec2(1280.0, 900.0));
    let row = harness.state().hit.rect("folder", to).unwrap();
    testing::click(&mut harness, row.center(), egui::PointerButton::Secondary, 1);
    harness.run_steps(2);
    harness.get_by_label("Rules for This Folder…").click();
    harness.run_steps(3);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else {
        panic!("the folder menu opens its rules")
    };
    assert_eq!(form.destination, Some(to));
    assert_eq!(harness.state().session.view.folder, from);
    harness.get_by_label("Close").click();
    harness.run_steps(3);
    // Favorites and the account tree may both represent this exact folder.
    // Either accessible control must name its destination and support Space.
    harness.query_all_by_label("Rules for Inbox - target@example.org").last().unwrap().focus();
    harness.run_steps(2);
    harness.key_press(egui::Key::Space);
    harness.run_steps(3);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else {
        panic!("the keyboard cog opens its rules")
    };
    assert_eq!(form.destination, Some(to));
    assert_eq!(harness.state().session.view.folder, from);
}

#[test]
fn destination_order_controls_use_the_full_incoming_account_order() {
    use egui_kittest::kittest::Queryable as _;
    let (mut app, from, to) = rendered_app();
    let mut hidden = app.session.mailbox.rules[0].clone();
    hidden.id = 502;
    hidden.name = "Another destination".into();
    hidden.actions = vec![Action::MoveTo(from)];
    let mut last = app.session.mailbox.rules[0].clone();
    last.id = 503;
    last.name = "Last invoice rule".into();
    app.session.mailbox.rules.extend([hidden, last]);
    app.execute("ui.dialog", json!({"name": "rules", "destination": to})).unwrap();
    let mut harness = window(app, vec2(1280.0, 900.0));
    harness.get_by_label("Move after Another destination in this account's complete rule order").click();
    harness.run_steps(3);
    assert_eq!(harness.state().session.mailbox.rules.iter().map(|r| r.id).collect::<Vec<_>>(), [502, 501, 503]);
    harness.get_by_label("Move after Last invoice rule in this account's complete rule order").click();
    harness.run_steps(3);
    assert_eq!(harness.state().session.mailbox.rules.iter().map(|r| r.id).collect::<Vec<_>>(), [502, 503, 501]);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else { panic!("the scoped list stays open") };
    assert_eq!(form.listed_rules(&harness.state().session.mailbox).iter().map(|r| r.id).collect::<Vec<_>>(), [503, 501]);
}

fn error_rect(harness: &egui_kittest::Harness<'_, crate::MailApp>) -> egui::Rect {
    use egui_kittest::kittest::Queryable as _;
    harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .find(|node| node.value().is_some_and(|value| value.contains("invalid or unsupported rule expression")))
        .unwrap()
        .rect()
}

#[test]
fn an_invalid_expression_explains_the_error_above_the_fixed_buttons_at_minimum_size() {
    use egui_kittest::kittest::Queryable as _;
    let (mut app, _, to) = rendered_app();
    app.execute("ui.dialog", json!({"name": "rules", "destination": to})).unwrap();
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &mut app.dialog else { panic!("the rule editor opens") };
    form.edit_rule(&app.session.mailbox, 501);
    form.conds[0].text = "[".into();
    form.preview = Some(json!({"matched": 1}));
    form.preview_is_dry_run = true;
    let original = app.session.mailbox.rules.clone();
    let size = vec2(760.0, 480.0);
    let mut harness = window(app, size);
    harness.get_by_label("Preview").click();
    harness.run_steps(4);
    let error = error_rect(&harness);
    assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(error));
    assert!(error.max.y <= harness.get_by_label("Save Rule").rect().min.y, "the error must remain above the fixed buttons");
    assert_eq!(harness.state().session.mailbox.rules, original);
    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &harness.state().dialog else { panic!("the original form stays open") };
    assert!(form.error.contains("unclosed character class"));
    assert!(form.preview.is_none(), "a failed preview must not retain an earlier successful count");
}

#[test]
fn editing_can_move_a_rule_to_another_destination_without_changing_its_identity() {
    let (mut session, source, from, to) = fixture();
    session.mailbox.rules.push(Rule {
        id: 501,
        name: "File invoices".into(),
        account: source,
        enabled: false,
        conditions: vec![Condition::HasAttachment],
        actions: vec![Action::MoveTo(to)],
        ..Default::default()
    });
    let mut form = RulesForm::new(&session, &json!({"destination": to}));
    form.edit_rule(&session.mailbox, 501);
    form.acts[0].folder = from;
    assert!(form.ready());
    let (command, params) = form.command();
    session.run(command, &params).unwrap();
    assert_eq!(session.mailbox.rules[0].id, 501);
    assert!(form.listed_rules(&session.mailbox).is_empty());
}

#[test]
fn editing_advanced_literal_preserves_whitespace_and_case_insensitive_mode() {
    let (mut session, source, _, to) = fixture();
    let text = RuleText { field: RuleField::Body, pattern: format!(" {} ", "x".repeat(250)), regex: false, case_sensitive: false };
    session.mailbox.rules.push(Rule {
        id: 501,
        name: "Literal".into(),
        account: source,
        conditions: vec![Condition::Text(text.clone())],
        actions: vec![Action::MoveTo(to)],
        ..Default::default()
    });
    let mut form = RulesForm::new(&session, &json!({"destination": to}));
    form.edit_rule(&session.mailbox, 501);
    let (command, params) = form.command();
    assert_eq!(params["conditions"][0], serde_json::to_value(Condition::Text(text)).unwrap());
    session.run(command, &params).unwrap();
    let rule = session.mailbox.rules.iter_mut().find(|rule| rule.id == 501).unwrap();
    rule.conditions = vec![Condition::Text(RuleText { field: RuleField::Body, pattern: "  ".into(), regex: false, case_sensitive: false })];
    form.edit_rule(&session.mailbox, 501);
    assert!(form.ready(), "a nonempty advanced literal preserves significant spaces");
    let (command, params) = form.command();
    session.run(command, &params).unwrap();
    assert_eq!(
        session.mailbox.rules[0].conditions[0],
        Condition::Text(RuleText { field: RuleField::Body, pattern: "  ".into(), regex: false, case_sensitive: false })
    );
}

#[test]
#[ignore = "Explicit offscreen native render, with task-owned output directory"]
fn shots_of_destination_rule_editor() {
    let directory = std::env::var("MAILCRAFT_RULE_SHOTS_DIR").unwrap();
    use egui_kittest::kittest::Queryable as _;
    for theme in ["light", "dark"] {
        for (width, height) in [(760.0, 480.0), (1024.0, 700.0), (1440.0, 900.0), (1920.0, 1080.0)] {
            for pixels in [1.0, 1.5] {
                let (mut app, _, to) = rendered_app();
                app.execute("ui.theme", json!({"value": theme})).unwrap();
                app.execute("ui.dialog", json!({"name": "rules", "destination": to})).unwrap();
                let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &mut app.dialog else {
                    panic!("the scoped editor must open")
                };
                form.edit_rule(&app.session.mailbox, 501);
                let mut harness = egui_kittest::Harness::builder()
                    .with_size(vec2(width, height))
                    .with_pixels_per_point(pixels)
                    .with_accessibility_check(false)
                    .wgpu()
                    .build_ui_state(
                        |ui, app: &mut crate::MailApp| {
                            let ctx = ui.ctx().clone();
                            app.logic(&ctx);
                            app.ui(ui);
                        },
                        app,
                    );
                harness.input_mut().max_texture_side = Some(8192);
                harness.run_steps(4);
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, height));
                for label in ["Save Rule", "Preview", "Cancel edit", "Close"] {
                    let rect = harness.get_by_label(label).rect();
                    assert!(viewport.contains_rect(rect), "{label} must remain visible at {width}x{height}, DPI {pixels}: {rect:?}");
                }
                let path = std::path::Path::new(&directory).join(format!("rules-{theme}-{}-{}.png", width as u32, (pixels * 100.0) as u32));
                harness.render().unwrap().save(&path).unwrap();
                println!("{}", path.display());
                if height == 480.0 {
                    harness.query_all_by_label("Regex").last().unwrap().scroll_to_me();
                    harness.run_steps(4);
                    let condition = harness.query_all_by_label("Regex").last().unwrap().rect();
                    assert!(viewport.contains_rect(condition), "the conditions must scroll into view");
                    assert!(condition.max.y <= harness.get_by_label("Save Rule").rect().min.y, "the footer must not cover the condition");
                    for label in ["Save Rule", "Preview", "Cancel edit", "Close"] {
                        assert!(viewport.contains_rect(harness.get_by_label(label).rect()), "scrolling must keep {label} visible");
                    }
                    let path =
                        std::path::Path::new(&directory).join(format!("rules-{theme}-{}-{}-scrolled.png", width as u32, (pixels * 100.0) as u32));
                    harness.render().unwrap().save(&path).unwrap();
                    println!("{}", path.display());
                    let original = harness.state().session.mailbox.rules.clone();
                    let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &mut harness.state_mut().dialog else {
                        panic!("the form stays open")
                    };
                    form.conds[0].text = "[".into();
                    harness.get_by_label("Preview").click();
                    harness.run_steps(4);
                    let error = error_rect(&harness);
                    assert!(viewport.contains_rect(error) && error.max.y <= harness.get_by_label("Save Rule").rect().min.y);
                    assert_eq!(harness.state().session.mailbox.rules, original);
                    for label in ["Save Rule", "Preview", "Cancel edit", "Close"] {
                        assert!(viewport.contains_rect(harness.get_by_label(label).rect()), "an error must keep {label} visible");
                    }
                    let path = std::path::Path::new(&directory).join(format!("rules-{theme}-{}-{}-error.png", width as u32, (pixels * 100.0) as u32));
                    harness.render().unwrap().save(&path).unwrap();
                    println!("{}", path.display());
                }
            }
        }
    }
}

#[test]
#[ignore = "retained synthetic render evidence; requires MAILCRAFT_RULE_SHOTS_DIR"]
fn shots_of_destination_rule_editor_merged_senders() {
    use egui_kittest::kittest::Queryable as _;
    let directory = std::env::var("MAILCRAFT_RULE_SHOTS_DIR").unwrap();
    for (theme, exact) in [("light", false), ("dark", false), ("light", true), ("dark", true)] {
        for (width, height) in [(760.0, 480.0), (1440.0, 900.0)] {
            for pixels in [1.0, 1.5] {
                let (mut app, _, to) = rendered_app();
                let senders = vec!["billing@example.org".into(), "receipts@example.org".into(), "news@example.org".into()];
                app.session.mailbox.rules[0].conditions = vec![if exact { Condition::FromAddresses(senders) } else { Condition::FromAny(senders) }];
                app.execute("ui.theme", json!({"value":theme})).unwrap();
                app.execute("ui.dialog", json!({"name":"rules", "destination":to})).unwrap();
                let Some(crate::dialogs::Dialog::Form(crate::forms::Form::Rules(form))) = &mut app.dialog else { panic!("the editor opens") };
                form.edit_rule(&app.session.mailbox, 501);
                let mut harness = egui_kittest::Harness::builder()
                    .with_size(vec2(width, height))
                    .with_pixels_per_point(pixels)
                    .with_accessibility_check(false)
                    .wgpu()
                    .build_ui_state(
                        |ui, app: &mut crate::MailApp| {
                            let ctx = ui.ctx().clone();
                            app.logic(&ctx);
                            app.ui(ui);
                        },
                        app,
                    );
                harness.input_mut().max_texture_side = Some(8192);
                harness.run_steps(4);
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, height));
                for label in ["Save Rule", "Preview", "Check conflicts", "Cancel edit", "Close"] {
                    assert!(viewport.contains_rect(harness.get_by_label(label).rect()));
                }
                let original = harness.state().session.mailbox.rules.clone();
                harness.get_by_label("Save Rule").click();
                harness.run_steps(3);
                assert_eq!(harness.state().session.mailbox.rules, original);
                harness.get_by_label("Edit this local rule").click();
                harness.run_steps(3);
                let suffix = if exact { "merged-exact" } else { "merged" };
                let path = std::path::Path::new(&directory).join(format!("rules-{theme}-{}-{}-{suffix}.png", width as u32, (pixels * 100.0) as u32));
                harness.render().unwrap().save(&path).unwrap();
                println!("{}", path.display());
            }
        }
    }
}
