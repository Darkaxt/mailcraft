// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The update switch sits in File › About under Check for Updates, and File › Options has none
//! (#488). Installing is the update window's Update now, never a switch.

use egui_kittest::Harness;
use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

/// A window on a build that can check for new versions (the answer never comes).
fn window() -> Harness<'static, MailApp> {
    let services = Services {
        update_check: Some(Box::new(|_| {
            let (_tx, rx) = std::sync::mpsc::channel();
            rx
        })),
        update_download: Some(Box::new(|_, _| {
            let (_tx, rx) = std::sync::mpsc::channel();
            rx
        })),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), 1_791_993_600, 0), services);
    app.autosave = false;
    let mut window = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    window.run_steps(3);
    window
}

fn show(window: &mut Harness<'static, MailApp>, page: &str) {
    window.state_mut().run("ui.backstage", json!({"value": true, "page": page})).unwrap();
    window.run_steps(4);
}

#[test]
fn about_holds_the_switch_below_check_for_updates() {
    let mut w = window();
    show(&mut w, "about");
    let check = w.state().hit.rect("checkForUpdates", 0).expect("Check for Updates");
    let option = w.state().hit.rect("updateOption", 0).expect("the check switch");
    assert!(option.top() >= check.bottom(), "the check switch is under the button");
    assert!(w.state().hit.rect("updateInstallOption", 0).is_none(), "no install switch");
}

#[test]
fn the_update_window_shows_how_much_of_the_download_is_there() {
    let running = mailcraft_model::update::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    let newer = mailcraft_model::update::Version { major: running.major + 1, minor: 0, patch: 0, suffix: String::new() };
    let found = mailcraft_model::update::Release { version: newer, page: String::new(), security: false, assets: Vec::new() };
    let services = Services {
        update_check: Some(Box::new(move |_| {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(Ok(found.clone()));
            rx
        })),
        // A quarter of 12 MB there, and the rest never comes.
        update_download: Some(Box::new(|_, progress| {
            progress.set(3_000_000, 12_000_000);
            let (tx, rx) = std::sync::mpsc::channel();
            std::mem::forget(tx);
            rx
        })),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), 1_791_993_600, 0), services);
    app.autosave = false;
    app.apply_prefs(crate::UiState { update_check: crate::updates::UpdateCheck::On, ..Default::default() });
    let mut w = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    w.run_steps(3);
    w.state_mut().run("ui.dialog", json!({"name": "update"})).unwrap();
    w.run_steps(3);
    w.state_mut().run("ui.restartToUpdate", json!({})).unwrap();
    w.run_steps(4);
    let state = w.state_mut().execute("ui.updateCheck", json!({"value": true})).unwrap();
    assert_eq!(state["install"]["progress"], json!({"received": 3_000_000, "total": 12_000_000}), "{state}");
    let bar = w.state().hit.rect("updateProgress", 0).expect("the download's bar");
    let cancel = w.state().hit.rect("updateCancel", 0).expect("Cancel");
    assert!(bar.width() > 100.0 && bar.bottom() <= cancel.top(), "the bar sits above Cancel: {bar:?} {cancel:?}");
}

#[test]
fn options_has_no_update_switch() {
    let mut w = window();
    show(&mut w, "options");
    assert!(w.state().hit.rect("updateOption", 0).is_none());
    assert!(w.state().hit.rect("updateInstallOption", 0).is_none());
}

#[test]
fn the_switch_in_about_turns_the_daily_check_on() {
    let mut w = window();
    show(&mut w, "about");
    let at = w.state().hit.rect("updateOption", 0).expect("the check switch").center();
    crate::people_view::testing::click(&mut w, at, egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert_eq!(w.state().ui.update_check, crate::updates::UpdateCheck::On);
}
