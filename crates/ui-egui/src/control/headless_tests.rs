// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Headless mail access never opens an interactive surface or grants consent.

use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::{ControlRequest, Outcome, handle};
use crate::{MailApp, Services};

fn request(app: &mut MailApp, method: &str, params: Value) -> Value {
    let request = ControlRequest::new(method, params).0.from_agent(Vec::new());
    let Outcome::Done(answer) = handle(app, &egui::Context::default(), &request) else {
        panic!("headless requests must not wait for an invisible owner question")
    };
    answer
}

#[test]
fn headless_status_and_mail_reads_work_without_window_or_graphics() {
    let mut app = MailApp::new(Session::new(mailcraft_engine::sample::mailbox(0, 0)), Services::default());
    app.start_headless();
    let status = request(&mut app, "app.status", json!({}));
    assert_eq!(status["result"]["lifecycle"]["mode"], "headless");
    assert_eq!(status["result"]["lifecycle"]["nativeWindow"], false);
    assert_eq!(status["result"]["lifecycle"]["nativeVisible"], Value::Null);
    assert_eq!(request(&mut app, "engine.execute", json!({"command": "account.list"}))["ok"], true);
    assert_eq!(request(&mut app, "engine.execute", json!({"command": "mail.list", "params": {"limit": 5}}))["ok"], true);
    assert!(app.dialog.is_none());
    assert!(app.asking.is_empty());
}

#[test]
fn headless_logic_does_not_warm_the_desktop_conversation_cache() {
    let mut app = MailApp::new(Session::new(mailcraft_engine::sample::mailbox(1, 12)), Services::default());
    app.start_headless();
    app.logic(&egui::Context::default());
    assert!(app.session.prepare_conversations(), "headless logic leaves desktop grouping lazy until it is actually requested");
    assert!(!app.session.prepare_conversations(), "the normal on-demand cache still works");
}

#[test]
fn headless_refuses_ui_and_owner_questions_instead_of_waiting() {
    let mut app = MailApp::new(Session::new(mailcraft_engine::sample::mailbox(0, 0)), Services::default());
    app.start_headless();
    for (method, params) in [
        ("ui.screenshot", json!({"inline": true})),
        ("ui.focus", json!({})),
        ("engine.execute", json!({"command": "ui.dialog", "params": {"name": "account"}})),
        ("engine.execute", json!({"command": "account.remove", "params": {"id": 1}})),
        ("engine.execute", json!({"command": "ui.allowRules", "params": {"account": 1, "confirmed": true}})),
    ] {
        let answer = request(&mut app, method, params);
        assert_eq!(answer["ok"], false, "{method}: {answer}");
    }
    assert!(app.dialog.is_none());
    assert!(app.asking.is_empty());
}

#[test]
fn unreadable_headless_mailbox_can_close_without_continuing_or_writing_it() {
    use crate::app_lifecycle::CloseReceipt;
    use crate::opening::{Arrival, Opening, Trouble};
    let (arrival, arrived) = std::sync::mpsc::channel();
    arrival.send(Arrival::unreadable(Trouble::Unreadable("synthetic damaged input".into()))).unwrap();
    let build = Box::new(|_| panic!("an unreadable mailbox must never be continued without its owner"));
    let mut app = MailApp::opening(Opening::new(arrived, Default::default(), build), None, None);
    app.start_headless();
    let ctx = egui::Context::default();
    app.logic(&ctx);
    assert!(app.opening_needs_attention());
    let (send, receive) = std::sync::mpsc::channel();
    app = app.with_control(receive);
    let receipt = CloseReceipt::default();
    let (mut close, reply) = ControlRequest::new("app.lifecycle", json!({"action": "close"}));
    close.close_receipt = Some(receipt.clone());
    send.send(close).unwrap();
    app.logic(&ctx);
    assert_eq!(reply.recv().unwrap()["ok"], true);
    receipt.complete(true);
    receipt.release();
    app.logic(&ctx);
    assert!(app.headless_close_ready());
    assert!(app.save_on_exit().is_ok());
    assert!(app.session.path.is_none());
}
