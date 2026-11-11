// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! App-owned lifecycle actions have no desktop input and cannot discard work.

use egui::ViewportCommand;
use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::{ControlRequest, Outcome, handle};
use crate::{MailApp, Services};

fn app() -> MailApp {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    app
}

fn call(app: &mut MailApp, ctx: &egui::Context, action: &str) -> Value {
    let request = ControlRequest::new("app.lifecycle", json!({"action": action})).0.from_agent(Vec::new());
    match handle(app, ctx, &request) {
        Outcome::Done(value) => value,
        _ => panic!("lifecycle actions answer without an owner-input dialog"),
    }
}

#[test]
fn hiding_uses_native_visibility_without_input_or_focus() {
    let mut app = app();
    let ctx = egui::Context::default();
    let mut answer = Value::Null;
    let mut output = ctx.run_ui(Default::default(), |ui| answer = call(&mut app, ui.ctx(), "hide"));
    assert_eq!(answer["ok"], true, "{answer}");
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(commands.contains(&ViewportCommand::Visible(false)));
    assert!(!commands.contains(&ViewportCommand::Focus));
    assert!(!app.quit_requested);
    output.textures_delta.clear();
}

#[test]
fn lifecycle_close_refuses_unsaved_compose_and_keeps_it_intact() {
    let mut app = app();
    app.session.run("mail.new", &json!({"to": "recipient@example.org", "body": "Keep this draft"})).unwrap();
    let before = app.session.compose.clone();
    let answer = call(&mut app, &egui::Context::default(), "close");
    assert_eq!(answer["ok"], false, "{answer}");
    assert!(answer["error"].as_str().unwrap().contains("message being written"), "{answer}");
    assert_eq!(app.session.compose, before);
    assert!(!app.quit_requested);
}

#[test]
fn lifecycle_never_accepts_force_or_discard_and_legacy_quit_stays_refused() {
    let mut app = app();
    let ctx = egui::Context::default();
    for params in [json!({"action": "close", "force": true}), json!({"action": "close", "discard": true}), json!({"action": "show"})] {
        let request = ControlRequest::new("app.lifecycle", params).0.from_agent(Vec::new());
        let Outcome::Done(answer) = handle(&mut app, &ctx, &request) else { panic!("unexpected pending action") };
        assert_eq!(answer["ok"], false);
        assert!(!app.quit_requested);
    }
    let request = ControlRequest::new("app.quit", json!({})).0.from_agent(Vec::new());
    let Outcome::Done(answer) = handle(&mut app, &ctx, &request) else { panic!("unexpected pending quit") };
    assert_eq!(answer["ok"], false);
}

#[test]
fn close_waits_for_delivered_reply_and_rechecks_actual_work() {
    for new_work in [false, true] {
        let mut app = app();
        let ctx = egui::Context::default();
        let receipt = crate::app_lifecycle::CloseReceipt::default();
        let mut request = ControlRequest::new("app.lifecycle", json!({"action": "close"})).0.from_agent(Vec::new());
        request.close_receipt = Some(receipt.clone());
        let Outcome::Done(answer) = handle(&mut app, &ctx, &request) else { panic!("unexpected pending action") };
        assert_eq!(answer["ok"], true, "{answer}");
        assert!(app.lifecycle_close_tick(&ctx));
        assert!(!app.quit_requested);
        if new_work {
            app.session.run("mail.new", &json!({"to": "recipient@example.org", "body": "Keep this new work"})).unwrap();
        }
        receipt.complete(true);
        assert!(app.lifecycle_close_tick(&ctx));
        assert!(!app.quit_requested, "a live bridge still needs its GUI during workflow release");
        receipt.release();
        assert_eq!(app.lifecycle_close_tick(&ctx), !new_work);
        assert_eq!(app.quit_requested, !new_work);
        if new_work {
            assert!(app.session.compose.is_some());
            assert_eq!(app.lifecycle.inspect()["blocked"], "a message being written must be saved and closed first");
        }
    }
}

#[test]
fn a_failed_reply_keeps_the_app_open_and_no_close_can_bypass_the_receipt() {
    let mut app = app();
    let ctx = egui::Context::default();
    let missing = call(&mut app, &ctx, "close");
    assert_eq!(missing["ok"], false);
    let receipt = crate::app_lifecycle::CloseReceipt::default();
    let mut request = ControlRequest::new("app.lifecycle", json!({"action": "close"})).0.from_agent(Vec::new());
    request.close_receipt = Some(receipt.clone());
    let Outcome::Done(answer) = handle(&mut app, &ctx, &request) else { panic!("unexpected pending action") };
    assert_eq!(answer["ok"], true);
    receipt.complete(false);
    assert!(!app.lifecycle_close_tick(&ctx));
    assert!(!app.quit_requested);
    assert!(app.lifecycle.inspect()["blocked"].as_str().unwrap().contains("not delivered"));
}

#[test]
fn a_queued_sync_blocks_close_without_cancelling_it() {
    let mut app = app();
    app.sync_queue.push_back((123, false));
    let result = call(&mut app, &egui::Context::default(), "close");
    assert_eq!(result["ok"], false);
    assert_eq!(app.sync_queue.front(), Some(&(123, false)));
    assert!(!app.quit_requested);
}

#[test]
fn another_workflow_can_resume_work_without_waiting_for_a_stale_close() {
    let mut app = app();
    let ctx = egui::Context::default();
    let receipt = crate::app_lifecycle::CloseReceipt::default();
    let mut request = ControlRequest::new("app.lifecycle", json!({"action": "close"})).0.from_agent(Vec::new());
    request.close_receipt = Some(receipt.clone());
    let Outcome::Done(answer) = handle(&mut app, &ctx, &request) else { panic!("unexpected pending action") };
    assert_eq!(answer["ok"], true);
    receipt.complete(true);
    let next = ControlRequest::new("engine.execute", json!({"command": "mail.new", "params": {"to": "next@example.org"}})).0.from_agent(Vec::new());
    let Outcome::Done(answer) = handle(&mut app, &ctx, &next) else { panic!("unexpected pending action") };
    assert_eq!(answer["ok"], true, "{answer}");
    receipt.release();
    assert!(!app.lifecycle_close_tick(&ctx));
    assert!(!app.quit_requested);
    assert!(app.session.compose.is_some());
    assert!(app.lifecycle.inspect()["blocked"].as_str().unwrap().contains("cancelled"));
}

#[test]
fn explicit_hidden_startup_emits_visibility_without_activation() {
    let mut app = app();
    app.start_hidden();
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(Default::default(), |ui| app.apply_background_visibility(ui.ctx()));
    let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
    assert!(commands.contains(&ViewportCommand::Visible(false)));
    assert!(!commands.contains(&ViewportCommand::Focus));
    output.textures_delta.clear();
}

#[derive(Default)]
struct SaveState {
    busy: bool,
    reports: Vec<mailcraft_engine::io::SaveReport>,
}

struct Saver(std::rc::Rc<std::cell::RefCell<SaveState>>);

impl mailcraft_engine::io::BackgroundSave for Saver {
    fn follow(&self, _: &std::path::Path) {}
    fn submit(&self, _: &std::path::Path, _: &mailcraft_model::Mailbox, _: u64) {}
    fn reports(&self) -> Vec<mailcraft_engine::io::SaveReport> {
        std::mem::take(&mut self.0.borrow_mut().reports)
    }
    fn progress(&self) -> Option<mailcraft_engine::io::SaveProgress> {
        self.0.borrow().busy.then_some(mailcraft_engine::io::SaveProgress {
            stage: mailcraft_engine::io::SaveStage::Messages,
            total: 1,
            ..Default::default()
        })
    }
    fn flush(&self) {}
}

#[test]
fn normal_mcp_close_waits_for_save_completion_and_save_failure_keeps_it_open() {
    for succeeds in [true, false] {
        let mut app = app();
        app.session.run("category.set", &json!({"name": "Lifecycle fixture", "color": "#aabbcc"})).unwrap();
        let rev = app.session.rev();
        assert!(rev > app.saved_rev);
        let path = std::path::PathBuf::from("memory-only.mailcraft");
        app.session.path = Some(path.clone());
        app.autosave = true;
        let save = std::rc::Rc::new(std::cell::RefCell::new(SaveState { busy: true, reports: Vec::new() }));
        app.services.saver = Some(Box::new(Saver(save.clone())));
        let ctx = egui::Context::default();
        let receipt = crate::app_lifecycle::CloseReceipt::default();
        let mut request = ControlRequest::new("app.lifecycle", json!({"action": "close"})).0.from_agent(Vec::new());
        request.close_receipt = Some(receipt.clone());
        let Outcome::Done(answer) = handle(&mut app, &ctx, &request) else { panic!("unexpected pending action") };
        assert_eq!(answer["ok"], true, "{answer}");
        receipt.complete(true);
        receipt.release();
        assert!(app.lifecycle_close_tick(&ctx));
        assert!(!app.close_requested(), "normal close must wait for the original save");
        assert_eq!(app.closing_after_save, Some(rev));
        save.borrow_mut().busy = false;
        save.borrow_mut().reports.push(mailcraft_engine::io::SaveReport {
            path,
            rev,
            result: if succeeds { Ok(()) } else { Err("synthetic disk full".into()) },
        });
        assert_eq!(app.lifecycle_close_tick(&ctx), succeeds);
        assert_eq!(app.quit_requested, succeeds);
        if succeeds {
            assert!(!app.session.dirty);
            assert!(app.close_requested());
        } else {
            assert!(app.session.dirty);
            assert!(app.lifecycle.inspect()["blocked"].as_str().unwrap().contains("save failed"));
        }
    }
}

#[test]
fn a_ready_search_coverage_result_does_not_block_close() {
    let mut app = app();
    let observed = crate::search_index_ui::observed(&app);
    app.search_index_pane = crate::search_index_ui::Pane::ready_for_test("search.gaps", app.session.openings(), observed);
    assert!(app.search_index_pane.holds_inspection());
    assert!(!app.search_index_pane.inspection_running());
    assert_eq!(app.lifecycle_blocker(), None);
    let result = call(&mut app, &egui::Context::default(), "close");
    assert_eq!(result["ok"], true, "{result}");
}

#[test]
fn an_unreleased_bridge_close_is_cancelled_after_a_bounded_wait() {
    let mut app = app();
    let ctx = egui::Context::default();
    let receipt = crate::app_lifecycle::CloseReceipt::default();
    let mut request = ControlRequest::new("app.lifecycle", json!({"action": "close"})).0.from_agent(Vec::new());
    request.close_receipt = Some(receipt.clone());
    let Outcome::Done(answer) = handle(&mut app, &ctx, &request) else { panic!("unexpected pending action") };
    assert_eq!(answer["ok"], true, "{answer}");
    receipt.complete(true);
    assert!(app.lifecycle_close_tick(&ctx), "logic waits while within the bound");
    let again = call(&mut app, &ctx, "close");
    assert_eq!(again["ok"], false, "a second close stays rejected while the first is pending");
    let past = crate::app_lifecycle::BRIDGE_RELEASE_WAIT + std::time::Duration::from_secs(1);
    app.lifecycle.pending_since = std::time::Instant::now().checked_sub(past);
    assert!(app.lifecycle.pending_since.is_some());
    assert!(!app.lifecycle_close_tick(&ctx), "normal logic resumes after the bound");
    assert!(!app.quit_requested);
    assert!(app.lifecycle.inspect()["blocked"].as_str().unwrap().contains("did not release"));
    assert_eq!(app.lifecycle.inspect()["close"], "idle");
}

#[test]
fn a_failed_headless_save_does_not_refuse_every_later_close() {
    let mut app = app();
    app.start_headless();
    app.headless_save_failed("synthetic disk full");
    assert!(!app.headless_close_ready());
    assert_eq!(app.lifecycle_blocker(), None, "the next headless close retries the save and reports it again");
}
