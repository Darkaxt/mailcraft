// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Headless, an agent's question is asked in the host's own window ([`crate::Services::ask_owner`])
//! and its answer goes through the same [`answer`] as a click over the desktop window: a yes runs
//! it, anything else is a no, an answer after the question ran out is thrown away, and the
//! questions are shown one at a time. The host here is a fake: no window opens.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};

use mailcraft_engine::Session;
use mailcraft_model::FolderKind;
use serde_json::{Value, json};

use crate::agent_gate::{OwnerQuestion, WAIT_MS, ask_owner_tick, expire};
use crate::control::{ControlRequest, Outcome};
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

/// The questions the fake host was asked, each with the sender its answer goes through.
type Shown = Rc<RefCell<Vec<(OwnerQuestion, Sender<bool>)>>>;

fn headless(host: bool) -> (MailApp, Shown) {
    let shown: Shown = Rc::default();
    let mut services = Services::default();
    if host {
        let asked = shown.clone();
        services.ask_owner = Some(Box::new(move |question| {
            let (tx, rx) = std::sync::mpsc::channel();
            asked.borrow_mut().push((question, tx));
            rx
        }));
    }
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    app.auto_sync = false;
    app.start_headless();
    (app, shown)
}

fn agent(app: &mut MailApp, command: &str, params: Value) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(app, &egui::Context::default(), &req.from_agent(Vec::new()));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

fn tick(app: &mut MailApp) {
    ask_owner_tick(app, &egui::Context::default());
}

fn write(app: &mut MailApp) {
    app.execute("mail.new", json!({"to": "x@attacker.example", "subject": "Re: Invoice"})).unwrap();
}

fn sent(app: &MailApp) -> usize {
    let out: Vec<u64> =
        app.session.mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Outbox | FolderKind::Sent)).map(|f| f.id).collect();
    app.session.mailbox.messages.values().filter(|m| out.contains(&m.folder) && m.subject == "Re: Invoice").count()
}

fn say(shown: &Shown, which: usize, yes: bool) {
    shown.borrow()[which].1.send(yes).unwrap();
}

#[test]
fn a_yes_in_the_hosts_window_sends() {
    let (mut a, shown) = headless(true);
    write(&mut a);
    let (now, rx) = agent(&mut a, "compose.send", json!({}));
    assert!(now.is_none(), "asked first");
    tick(&mut a);
    {
        let asked = shown.borrow();
        assert_eq!(asked.len(), 1);
        let q = &asked[0].0;
        assert!(q.text.starts_with("An agent wants to ") && q.text.contains("Re: Invoice"), "{}", q.text);
        assert_eq!((q.yes.as_str(), q.no.as_str()), ("Send", "Don't Send"));
        assert!(q.seconds > 0 && (q.seconds as f64) < WAIT_MS / 1000.0, "{}", q.seconds);
        assert!(!q.note.is_empty());
    }
    // Nothing yet: still waiting for the owner.
    tick(&mut a);
    assert!(rx.try_recv().is_err());
    assert_eq!(sent(&a), 0);
    say(&shown, 0, true);
    tick(&mut a);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert_eq!(sent(&a), 1);
    assert!(a.asking.is_empty());
    assert!(a.owner_window.is_none());
}

#[test]
fn a_no_or_a_window_gone_without_answering_sends_nothing() {
    let (mut a, shown) = headless(true);
    write(&mut a);
    let (_, rx) = agent(&mut a, "compose.send", json!({}));
    tick(&mut a);
    say(&shown, 0, false);
    tick(&mut a);
    assert!(rx.try_recv().unwrap()["error"].as_str().unwrap().contains("the owner said no"));
    assert_eq!(sent(&a), 0);
    // The host went without an answer (its sender dropped): no.
    let (_, rx) = agent(&mut a, "compose.send", json!({}));
    tick(&mut a);
    assert_eq!(shown.borrow().len(), 2);
    shown.borrow_mut().pop();
    tick(&mut a);
    assert!(rx.try_recv().unwrap()["error"].as_str().unwrap().contains("the owner said no"));
    assert_eq!(sent(&a), 0);
    assert!(a.session.compose.is_some());
}

#[test]
fn without_a_host_headless_questions_are_refused_as_before() {
    let (mut a, _) = headless(false);
    write(&mut a);
    let (now, _) = agent(&mut a, "compose.send", json!({}));
    let now = now.expect("refused at once");
    assert_eq!(now["ok"], false);
    assert!(now["error"].as_str().unwrap().contains("owner interaction required in the desktop"), "{now}");
    assert!(a.asking.is_empty());
    assert_eq!(sent(&a), 0);
}

#[test]
fn an_answer_after_the_question_ran_out_is_thrown_away() {
    let (mut a, shown) = headless(true);
    write(&mut a);
    let (_, rx) = agent(&mut a, "compose.send", json!({}));
    tick(&mut a);
    expire(&mut a, crate::now_ms() + WAIT_MS + 1.0);
    assert!(rx.try_recv().unwrap()["error"].as_str().unwrap().contains("in time"));
    // The owner clicks yes too late.
    say(&shown, 0, true);
    tick(&mut a);
    assert_eq!(sent(&a), 0);
    assert!(rx.try_recv().is_err(), "one answer only");
    assert!(shown.borrow()[0].0.cancel.load(Ordering::Relaxed), "the host's window is told to close");
    assert!(a.owner_window.is_none());
}

#[test]
fn questions_are_shown_one_after_the_other() {
    let (mut a, shown) = headless(true);
    write(&mut a);
    let outbox = a.session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).unwrap().id;
    let id = *a.session.mailbox.messages.keys().next().unwrap();
    let (_, first) = agent(&mut a, "compose.send", json!({}));
    let (_, second) = agent(&mut a, "mail.copy", json!({"folder": outbox, "ids": [id]}));
    assert_eq!(a.asking.len(), 2);
    tick(&mut a);
    tick(&mut a);
    assert_eq!(shown.borrow().len(), 1, "one window at a time");
    say(&shown, 0, false);
    tick(&mut a);
    assert!(first.try_recv().unwrap()["error"].as_str().unwrap().contains("the owner said no"));
    assert_eq!(shown.borrow().len(), 2, "the next question comes up");
    assert!(second.try_recv().is_err());
    say(&shown, 1, false);
    tick(&mut a);
    assert!(second.try_recv().unwrap()["error"].as_str().unwrap().contains("the owner said no"));
    assert_eq!(a.session.mailbox.count(outbox), 0);
    assert_eq!(sent(&a), 0);
    assert!(a.owner_window.is_none());
}
