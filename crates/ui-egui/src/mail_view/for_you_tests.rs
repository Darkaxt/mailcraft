// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you's heading in the list, Folder & subject rows, and the reading pane hidden while
//! nothing is selected.

use mailcraft_engine::Session;
use mailcraft_model::Address;
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.overlay_scroll_bars = false;
    app
}

fn window(app: MailApp) -> Window {
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(1280.0, 900.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    harness
}

fn click_at(harness: &mut Window, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

fn drawn(harness: &mut Window) -> Vec<(Row, Rect)> {
    let rows = harness.state_mut().session.rows();
    rows.iter().filter_map(|row| harness.state().hit.rect("message", row.id).map(|rect| (row.clone(), rect))).collect()
}

fn folder_named(app: &MailApp, name: &str) -> Id {
    app.session.mailbox.folders.iter().find(|f| f.name == name).map(|f| f.id).unwrap()
}

/// An unread message of today in `folder`, from someone never heard from before.
fn new_mail(app: &mut MailApp, folder: Id, from: &str) -> Id {
    let at = NOW - 3_600;
    let m = Message {
        folder,
        from: Address { name: String::new(), email: from.into() },
        subject: format!("Hello from {from}"),
        date: at,
        received: at,
        read: true,
        ..Default::default()
    };
    let id = app.session.mailbox.add_message(m).unwrap();
    // (A read mark alone carries the list through as it was: the mailbox changed first.)
    app.session.mailbox_changed();
    app.execute("mail.markUnread", json!({"ids": [id]})).unwrap();
    id
}

fn for_you_on(app: &mut MailApp) {
    app.execute("forYou.settings", json!({"allMail": true, "inbox": true})).unwrap();
    app.execute("view.allMail", json!({})).unwrap();
}

#[test]
fn the_header_shows_for_you_and_opens_the_settings() {
    let mut a = app();
    // The sample's own unread mail read, so that the section holds only the message added (it
    // would otherwise be among many newer ones, past the cap or the window's bottom).
    let ids: Vec<Id> = a.session.mailbox.messages.values().map(|m| m.id).collect();
    for id in ids {
        a.session.mailbox.message_mut(id).unwrap().read = true;
    }
    a.session.mailbox_changed();
    for_you_on(&mut a);
    let inbox = a.session.mailbox.inbox().unwrap();
    let new = new_mail(&mut a, inbox, "ana@people.example");
    let mut harness = window(a);
    let rows = drawn(&mut harness);
    assert!(rows.iter().any(|(r, _)| r.id == new && r.for_you.is_some()));
    assert_eq!(rows.first().and_then(|(r, _)| r.group.as_deref()), Some(mailcraft_engine::for_you::HEADING));
    let gear = harness.state().hit.rect("forYouGear", 0).expect("the section's gear");
    assert!(!harness.state().for_you_settings.open);
    click_at(&mut harness, gear.center());
    assert!(harness.state().for_you_settings.open);
    harness.run_steps(3);
    assert!(harness.state().for_you_settings.open);
}

#[test]
fn show_all_appears_only_when_rows_are_capped() {
    let mut a = app();
    for_you_on(&mut a);
    let inbox = a.session.mailbox.inbox().unwrap();
    for who in ["ana@people.example", "bo@people.example", "cy@people.example"] {
        new_mail(&mut a, inbox, who);
    }
    a.execute("forYou.settings", json!({"cap": 1})).unwrap();
    let mut harness = window(a);
    let show_all = harness.state().hit.rect("forYouShowAll", 0).expect("Show all while rows are held back");
    click_at(&mut harness, show_all.center());
    assert!(harness.state().session.view.for_you_all);
    // Show fewer, then no cap: nothing to show or hide.
    harness.state_mut().execute("forYou.showAll", json!({"value": false})).unwrap();
    harness.state_mut().execute("forYou.settings", json!({"cap": 0})).unwrap();
    harness.run_steps(3);
    assert!(harness.state().hit.rect("forYouShowAll", 0).is_none());
    assert!(harness.state().hit.rect("forYouGear", 0).is_some());
}

#[test]
fn folder_rows_lead_with_the_folder_and_hide_the_sender() {
    let mut a = app();
    let receipts = folder_named(&a, "Receipts");
    new_mail(&mut a, receipts, "shop@store.example");
    a.execute("view.previewLines", json!({"value": 3})).unwrap();
    a.execute("view.rowStyle", json!({"value": "folder"})).unwrap();
    a.execute("folder.select", json!({"id": receipts})).unwrap();
    let mut harness = window(a);
    let rows = drawn(&mut harness);
    assert!(!rows.is_empty());
    for (row, rect) in &rows {
        // Two lines and no preview, whatever the preview lines say.
        assert!(rect.height() <= NORMAL_H && rect.height() == rows[0].1.height(), "{}", row.id);
        let folder = harness.state().hit.rect("messageFolder", row.id).expect("the folder on top");
        let subject = harness.state().hit.rect("messageSubject", row.id).expect("the subject");
        assert!(folder.center().y < subject.center().y);
        assert!(harness.state().hit.rect("messageSender", row.id).is_none());
    }
}

#[test]
fn an_inbox_message_shows_its_sender_on_top() {
    let mut a = app();
    a.execute("view.rowStyle", json!({"value": "folder"})).unwrap();
    let inbox = a.session.mailbox.inbox().unwrap();
    a.execute("folder.select", json!({"id": inbox})).unwrap();
    let mut harness = window(a);
    let rows = drawn(&mut harness);
    assert!(!rows.is_empty());
    for (row, _) in &rows {
        let sender = harness.state().hit.rect("messageSender", row.id).expect("the sender on top");
        let subject = harness.state().hit.rect("messageSubject", row.id).expect("the subject");
        assert!(sender.center().y < subject.center().y);
        assert!(harness.state().hit.rect("messageFolder", row.id).is_none());
    }
}

fn settings_open(a: MailApp) -> Window {
    let mut harness = window(a);
    harness.state_mut().for_you_settings.open = true;
    harness.run_steps(4);
    harness
}

#[test]
fn dragging_a_number_settles_it_once_the_drag_ends() {
    let mut harness = settings_open(app());
    let cap = harness.state().session.prefs.for_you.cap;
    let at = harness.state().hit.rect("forYouCap", 0).expect("the cap's number").center();
    harness.event(egui::Event::PointerMoved(at));
    harness.step();
    harness.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    harness.step();
    let mut to = at;
    for _ in 0..10 {
        to.x += 6.0;
        harness.event(egui::Event::PointerMoved(to));
        harness.step();
    }
    // Mid-drag the list is not recounted: the setting waits.
    assert_eq!(harness.state().session.prefs.for_you.cap, cap);
    harness.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    harness.run_steps(4);
    assert!(harness.state().session.prefs.for_you.cap > cap);
}

#[test]
fn clearing_the_link_history_asks_first() {
    let mut a = app();
    let first = a.session.mailbox.messages.values().map(|m| m.id).next().unwrap();
    a.session.note_link_opened(first);
    let mut harness = settings_open(a);
    let clear = harness.state().hit.rect("forYouClearLinks", 0).expect("Clear link history");
    click_at(&mut harness, clear.center());
    assert!(harness.state().session.mailbox.message(first).unwrap().link_opened.is_some());
    // Cancel keeps it...
    let cancel = harness.state().hit.rect("forYouClearCancel", 0).expect("Cancel");
    click_at(&mut harness, cancel.center());
    assert!(harness.state().hit.rect("forYouClearConfirm", 0).is_none());
    assert!(harness.state().session.mailbox.message(first).unwrap().link_opened.is_some());
    // ...Clear forgets it.
    let clear = harness.state().hit.rect("forYouClearLinks", 0).unwrap();
    click_at(&mut harness, clear.center());
    let confirm = harness.state().hit.rect("forYouClearConfirm", 0).expect("Clear");
    click_at(&mut harness, confirm.center());
    assert!(harness.state().session.mailbox.message(first).unwrap().link_opened.is_none());
}

fn list_width(harness: &mut Window) -> f32 {
    drawn(harness).first().map(|(_, r)| r.width()).expect("a row")
}

#[test]
fn with_nothing_selected_the_list_takes_the_whole_width() {
    let mut a = app();
    a.execute("view.readingPane", json!({"value": "right"})).unwrap();
    a.execute("view.paneAutoHide", json!({"value": true})).unwrap();
    a.execute("mail.select", json!({"ids": []})).unwrap();
    let mut harness = window(a);
    let wide = list_width(&mut harness);
    assert!(wide > 800.0, "{wide}");
    // A click selects at once; the pane comes up after a double click would have ended, with the
    // row where it was, under the pointer.
    let (row, rect) = drawn(&mut harness).first().cloned().unwrap();
    let at = pos2(rect.min.x + 120.0, rect.center().y);
    click_at(&mut harness, at);
    assert_eq!(harness.state().session.sel.messages, vec![row.id]);
    assert_eq!(list_width(&mut harness), wide);
    harness.run_steps(40);
    let narrow = harness.state().hit.rect("message", row.id).unwrap();
    assert!(narrow.width() < 600.0, "{}", narrow.width());
    assert!(narrow.contains(at));
    // Nothing selected again: hidden again.
    harness.state_mut().execute("mail.select", json!({"ids": []})).unwrap();
    harness.run_steps(3);
    assert_eq!(list_width(&mut harness), wide);
}

#[test]
fn the_auto_hide_never_shows_an_off_pane() {
    let mut a = app();
    a.execute("view.readingPane", json!({"value": "off"})).unwrap();
    a.execute("view.paneAutoHide", json!({"value": true})).unwrap();
    let mut harness = window(a);
    let first = drawn(&mut harness).first().map(|(r, _)| r.id).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": first})).unwrap();
    harness.run_steps(40);
    assert!(list_width(&mut harness) > 800.0);
    assert_eq!(harness.state().session.view.reading_pane, Pane::Off);
}

#[test]
fn turned_off_the_pane_stays_with_nothing_selected() {
    let mut a = app();
    a.execute("view.readingPane", json!({"value": "right"})).unwrap();
    a.execute("mail.select", json!({"ids": []})).unwrap();
    let mut harness = window(a);
    assert!(list_width(&mut harness) < 600.0);
}
