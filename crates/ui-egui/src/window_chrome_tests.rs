// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn only_blank_header_space_is_a_drag_region() {
    let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(760.0, 48.0));
    let strip = drag_strip(full, 140.0, 520.0);
    assert_eq!(strip.len(), 1);
    assert!(!strip[0].contains(pos2(50.0, 24.0)));
    assert!(strip[0].contains(pos2(300.0, 24.0)));
    assert!(!strip[0].contains(pos2(650.0, 24.0)));
    assert!(drag_strip(full, 520.0, 140.0).is_empty());
}

#[test]
fn headless_window_actions_never_create_or_show_a_window() {
    let mut app = MailApp::new(mailcraft_engine::Session::empty(), crate::Services::default());
    app.start_headless();
    for id in ["ui.minimizeMainWindow", "ui.toggleMaximizeMainWindow", "ui.closeMainWindow"] {
        assert!(command(&mut app, id).is_err());
        assert!(app.is_headless());
    }
}

#[test]
fn agent_window_actions_reach_the_headless_refusal_without_an_owner_question() {
    let mut app = MailApp::new(mailcraft_engine::Session::empty(), crate::Services::default());
    app.start_headless();
    let ctx = egui::Context::default();
    for id in ["ui.minimizeMainWindow", "ui.toggleMaximizeMainWindow", "ui.closeMainWindow"] {
        let (request, _reply) = crate::control::ControlRequest::new("engine.execute", json!({"command":id,"params":{}}));
        let request = request.from_agent(Vec::new());
        let crate::control::Outcome::Done(result) = crate::control::handle(&mut app, &ctx, &request) else {
            panic!("a headless refusal must complete without asking or rendering");
        };
        assert_eq!(result["ok"], false);
        assert!(result["error"].as_str().unwrap().contains("headless mode never opens a window"), "{result}");
        assert!(app.asking.is_empty());
        assert!(app.is_headless());
    }
}

#[test]
fn caption_actions_cannot_reveal_a_hidden_desktop_window() {
    let mut h = crate::people_view::testing::window("view.mail");
    h.state_mut().start_hidden();
    for id in ["ui.minimizeMainWindow", "ui.toggleMaximizeMainWindow"] {
        assert!(command(h.state_mut(), id).unwrap_err().contains("hidden"));
    }
    assert!(command(h.state_mut(), "ui.closeMainWindow").is_ok(), "normal hidden close stays available");
}

#[test]
fn integrated_header_keeps_search_and_caption_controls_separate_in_both_themes() {
    for width in [760.0, 900.0, 1440.0] {
        for theme in [crate::theme::Appearance::Light, crate::theme::Appearance::Dark] {
            let mut app = MailApp::new(mailcraft_engine::Session::empty(), crate::Services::default());
            app.windows_titlebar = true;
            app.ui.theme = theme;
            app.autosave = false;
            let mut h =
                egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(width, 800.0)).build_ui_state(
                    |ui, app: &mut MailApp| {
                        let ctx = ui.ctx().clone();
                        app.logic(&ctx);
                        app.ui(ui);
                    },
                    app,
                );
            h.run_steps(5);
            let app = h.state();
            let close = app.hit.rect("windowCaption", 2).unwrap();
            let maximize = app.hit.rect("windowCaption", 1).unwrap();
            let minimize = app.hit.rect("windowCaption", 0).unwrap();
            assert!(close.right() <= width + 0.5);
            assert!(minimize.right() <= maximize.left() + 0.5);
            assert!(maximize.right() <= close.left() + 0.5);
            assert_eq!(app.window_header.maximize, Some(maximize));
            assert!(app.search_box_shown, "search must remain available at {width}");
            let search = app.window_header.exclusions.first().unwrap();
            assert!(!search.intersects(minimize) && !search.intersects(maximize) && !search.intersects(close));
            h.state_mut().execute("ui.backstage", json!({"value":true})).unwrap();
            h.run_steps(2);
            assert!(h.state().hit.rect("windowCaption", 2).is_some(), "Backstage keeps normal close available");
        }
    }
}
