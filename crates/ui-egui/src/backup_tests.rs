// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backups go only where the owner chose (0.12.11): off until turned on, never into a folder
//! MailCraft picked itself, and a bar while those that are on can't be made.

use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use crate::MailApp;
use crate::people_view::testing;

fn window(on: bool, folder: Option<&str>) -> egui_kittest::Harness<'static, MailApp> {
    let mut w = testing::window("view.mail");
    {
        let app = w.state_mut();
        app.session.path = Some(std::env::temp_dir().join("mailcraft-backup-ui-test.mailcraft"));
        app.ui.backups = on;
        app.ui.backup_folder = folder.map(str::to_string);
    }
    w.run_steps(3);
    w
}

#[test]
fn off_and_with_no_folder_until_the_owner_chooses() {
    assert!(!crate::UiState::default().backups, "a first start has no backups");
    let mut w = window(false, None);
    let status = w.state_mut().execute("ui.backupStatus", json!({})).unwrap();
    assert_eq!((status["on"].clone(), status["folder"].clone(), status["trouble"].clone()), (json!(false), json!(null), json!(null)));
    assert!(w.query_by_label_contains("Backups are on").is_none());
    // Turning them on without a folder, where the owner can't be asked for one, does nothing.
    assert!(w.state_mut().execute("ui.backups", json!({"value": true})).is_err());
    assert!(!w.state().ui.backups);
}

#[test]
fn on_with_no_folder_says_so_and_writes_nothing() {
    let mut w = window(true, None);
    assert!(w.query_by_label_contains("no folder is chosen").is_some(), "the bar");
    assert!(w.state_mut().execute("ui.backupNow", json!({})).is_err());
    assert!(w.state_mut().execute("ui.backupStatus", json!({})).unwrap()["trouble"].is_string());
}

#[test]
fn a_folder_that_is_not_there_is_said_and_not_made() {
    let gone = std::env::temp_dir().join(format!("mailcraft-backup-gone-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&gone);
    let mut w = window(true, Some(&gone.display().to_string()));
    assert!(w.query_by_label_contains("isn't there").is_some(), "the bar");
    assert!(w.state_mut().execute("ui.backupNow", json!({})).is_ok_and(|r| r["started"] == true));
    w.run_steps(30);
    assert!(!gone.exists(), "never made again");
    // Choosing a folder that is there puts the bar away.
    let here = std::env::temp_dir();
    w.state_mut().execute("ui.backupFolder", json!({"path": here.display().to_string()})).unwrap();
    w.run_steps(2);
    assert!(w.query_by_label_contains("isn't there").is_none());
}
