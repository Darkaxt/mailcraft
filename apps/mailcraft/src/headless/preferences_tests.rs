// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Changed headless engine options survive without replacing desktop settings.

use std::path::PathBuf;

use mailcraft_engine::Session;
use mailcraft_ui_egui::{MailApp, Services, UiState};
use serde_json::{Value, json};

use super::save_options_at;

pub(super) struct Settings(pub PathBuf);

impl Settings {
    pub(super) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("mailcraft-headless-prefs-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.join("ui.json"))
    }

    pub(super) fn read(&self) -> Value {
        serde_json::from_slice(&std::fs::read(&self.0).unwrap()).unwrap()
    }
}

impl Drop for Settings {
    fn drop(&mut self) {
        std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
    }
}

fn app() -> MailApp {
    let mut app = MailApp::new(Session::new(mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org")), Services::default());
    app.auto_sync = false;
    app.autosave = false;
    app.start_headless();
    app
}

#[test]
fn changed_options_survive_reopen_without_replacing_desktop_or_unknown_fields() {
    let settings = Settings::new("merge");
    let mut app = app();
    let initial = app.session.prefs.clone();
    let desktop = json!({"theme":"dark", "listWidth":593, "futureLayout":{"keep":[1,2]},
        "view":{"sort":"subject"}, "options":{"offline":false, "previewLines":3, "futureOption":{"keep":true}}});
    std::fs::write(&settings.0, serde_json::to_vec(&desktop).unwrap()).unwrap();
    app.session.run("sync.offline", &json!({"value":true})).unwrap();
    save_options_at(&app, &initial, &settings.0).unwrap();
    let mut expected = desktop;
    expected["options"]["offline"] = json!(true);
    assert_eq!(settings.read(), expected);
    let saved: UiState = serde_json::from_slice(&std::fs::read(&settings.0).unwrap()).unwrap();
    let mut reopened = self::app();
    reopened.apply_prefs(saved);
    assert_eq!(reopened.session.run("file.account", &json!({})).unwrap()["offline"], true);
}

#[test]
fn an_unchanged_windowless_check_does_not_rewrite_settings() {
    let settings = Settings::new("unchanged");
    let mut app = app();
    let mut initial = app.session.prefs.clone();
    initial.follow_conventions(app.date_conventions());
    let bytes = b"{\n  \"futureLayout\": true\n}\n";
    std::fs::write(&settings.0, bytes).unwrap();
    let (ctx, _) = super::context();
    ctx.begin_pass(egui::RawInput::default());
    app.logic(&ctx);
    let mut output = ctx.end_pass();
    output.textures_delta.clear();
    save_options_at(&app, &initial, &settings.0).unwrap();
    assert_eq!(std::fs::read(&settings.0).unwrap(), bytes);
}

#[test]
fn new_settings_keep_fresh_defaults_and_removed_data_is_not_recreated() {
    let settings = Settings::new("fresh");
    let mut app = app();
    app.fresh_prefs();
    let initial = app.session.prefs.clone();
    app.session.run("sync.offline", &json!({"value":true})).unwrap();
    app.data_removed = true;
    save_options_at(&app, &initial, &settings.0).unwrap();
    assert!(!settings.0.exists());
    app.data_removed = false;
    save_options_at(&app, &initial, &settings.0).unwrap();
    let saved: UiState = serde_json::from_slice(&std::fs::read(&settings.0).unwrap()).unwrap();
    assert_eq!(saved.options, app.session.prefs);
}

#[test]
fn malformed_settings_and_write_failure_are_reported_without_overwriting() {
    let settings = Settings::new("failure");
    let mut app = app();
    let initial = app.session.prefs.clone();
    app.session.run("sync.offline", &json!({"value":true})).unwrap();
    for bytes in [b"{damaged".as_slice(), b"[]", b"{\"options\": false}"] {
        std::fs::write(&settings.0, bytes).unwrap();
        let why = save_options_at(&app, &initial, &settings.0).unwrap_err();
        assert!(why.contains("preferences could not be saved"));
        assert_eq!(std::fs::read(&settings.0).unwrap(), bytes);
    }
    let impossible = settings.0.join("ui.json");
    let why = save_options_at(&app, &initial, &impossible).unwrap_err();
    assert!(why.contains("preferences could not be saved"));
    app.headless_save_failed(&why);
    assert!(!app.headless_close_ready(), "failed persistence must retain the native owner");
}
