// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account whose sign-in ran out is kept as needing one until a sign-in succeeds: `status`
//! says so, and headless the owner is asked once, in the host's window, never over an agent's
//! question. The host here is a fake: no window opens and no page either.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use mailcraft_engine::Session;
use mailcraft_model::Id;

use crate::agent_gate::OwnerQuestion;
use crate::sign_in_notice::{Trouble, needs_new_sign_in, tick};
use crate::{MailApp, Services, approval};

const NOW: i64 = 1_791_993_600;

type Shown = Rc<RefCell<Vec<(OwnerQuestion, Sender<bool>)>>>;

fn headless() -> (MailApp, Shown, Id) {
    let shown: Shown = Rc::default();
    let mut services = Services::default();
    let asked = shown.clone();
    services.ask_owner = Some(Box::new(move |question| {
        let (tx, rx) = std::sync::mpsc::channel();
        asked.borrow_mut().push((question, tx));
        rx
    }));
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    app.auto_sync = false;
    app.start_headless();
    let account = app.session.mailbox.accounts.first().expect("the sample has an account").id;
    (app, shown, account)
}

fn step(app: &mut MailApp) {
    tick(app, &egui::Context::default());
}

#[test]
fn only_the_errors_that_need_a_new_sign_in_count() {
    assert!(needs_new_sign_in(approval::GOOGLE_REAUTH_REQUIRED));
    assert!(needs_new_sign_in(approval::GOOGLE_NEEDS_SIGN_IN));
    let ran_out = format!("{}avery@lumen.example{}", approval::SIGN_IN_RAN_OUT_START, approval::SIGN_IN_RAN_OUT_END);
    assert!(needs_new_sign_in(&ran_out));
    for other in [
        approval::SIGN_IN_DECLINED,
        approval::SIGN_IN_TIMED_OUT,
        approval::NO_BROWSER,
        "Google sign-in could not reach the authorization server. Retry later.",
    ] {
        assert!(!needs_new_sign_in(other), "{other}");
    }
}

#[test]
fn the_owner_is_asked_once_until_signed_in_again() {
    let (mut app, shown, account) = headless();
    step(&mut app);
    assert!(shown.borrow().is_empty(), "nothing to ask about");

    app.note_sign_in_error(account, approval::GOOGLE_NEEDS_SIGN_IN);
    assert_eq!(app.sign_in_needed(), vec![account]);
    assert_eq!(app.lifecycle_status()["signInNeeded"][0]["account"], serde_json::json!(account));
    step(&mut app);
    assert_eq!(shown.borrow().len(), 1);
    {
        let q = &shown.borrow()[0].0;
        assert_eq!((q.yes.as_str(), q.no.as_str()), ("Sign In", "Later"));
        assert!(q.preview.is_none());
        assert!(q.seconds >= 60, "a minute at least");
    }

    // Later: not asked again while it still needs the sign-in.
    shown.borrow()[0].1.send(false).unwrap();
    for _ in 0..3 {
        step(&mut app);
    }
    assert_eq!(shown.borrow().len(), 1);
    assert_eq!(app.owner_sign_in, None);
    assert_eq!(app.sign_in_needed(), vec![account], "Later doesn't make it signed in");

    // Signed in again: no longer needed, and asked again should it run out once more.
    app.note_signed_in(account);
    assert!(app.sign_in_needed().is_empty());
    assert!(app.lifecycle_status()["signInNeeded"].as_array().unwrap().is_empty());
    app.note_sign_in_error(account, approval::GOOGLE_REAUTH_REQUIRED);
    step(&mut app);
    assert_eq!(shown.borrow().len(), 2);
}

#[test]
fn a_declined_or_timed_out_sign_in_is_not_marked() {
    let (mut app, shown, account) = headless();
    app.note_sign_in_error(account, approval::SIGN_IN_TIMED_OUT);
    app.note_sign_in_error(account, "Google sign-in could not reach the authorization server. Retry later.");
    step(&mut app);
    assert!(app.sign_in_needed().is_empty());
    assert!(shown.borrow().is_empty());
}

#[test]
fn sign_in_takes_the_owners_click_to_a_send_receive() {
    let (mut app, shown, account) = headless();
    app.note_sign_in_error(account, approval::GOOGLE_NEEDS_SIGN_IN);
    step(&mut app);
    shown.borrow()[0].1.send(true).unwrap();
    // The answer is read; this build has no sync service, so the Send/Receive can't start and
    // the click is dropped rather than kept for ever.
    step(&mut app);
    assert_eq!(app.owner_sign_in, None);
    assert_eq!(app.sign_in_needed(), vec![account], "still needed until a sign-in succeeds");
    assert_eq!(shown.borrow().len(), 1, "asked once");
}

#[test]
fn never_over_an_agents_question() {
    let (mut app, shown, account) = headless();
    // An agent's question showing: the sign-in question waits.
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (_tx, rx) = std::sync::mpsc::channel();
    app.note_sign_in_error(account, approval::GOOGLE_NEEDS_SIGN_IN);
    app.owner_window = Some(crate::agent_gate::OwnerWindow::for_tests(rx, cancel));
    step(&mut app);
    assert!(shown.borrow().is_empty());
    app.owner_window = None;
    step(&mut app);
    assert_eq!(shown.borrow().len(), 1);
}

/// A message sent from such an account says it waits in the Outbox, and why.
#[test]
fn a_send_from_it_says_it_waits() {
    let (mut app, _shown, _) = headless();
    app.execute("mail.new", serde_json::json!({"to": "x@example.org", "subject": "s"})).unwrap();
    let sent = app.execute("compose.send", serde_json::json!({})).unwrap();
    assert!(sent.get("waits").is_none(), "{sent}");
    let id = sent["id"].as_u64().unwrap();
    let account = app.session.mailbox.message(id).unwrap().account;
    app.note_sign_in_error(account, approval::GOOGLE_NEEDS_SIGN_IN);
    let r = app.note_waiting_send("compose.send", Ok(serde_json::json!({"id": id})));
    assert!(r.unwrap()["waits"].as_str().unwrap().contains("sign in again"));
}

/// The title bar's shield lists what needs the owner, account by account, once each.
#[test]
fn the_shield_lists_each_account_once() {
    let (mut app, _shown, account) = headless();
    assert!(app.account_troubles().is_empty());
    app.note_sign_in_error(account, approval::GOOGLE_NEEDS_SIGN_IN);
    assert_eq!(app.account_troubles().iter().map(|(a, _, t)| (*a, *t)).collect::<Vec<_>>(), vec![(account, Trouble::SignIn)]);
    app.note_signed_in(account);
    assert!(app.account_troubles().is_empty());
}

/// A sign-in waiting for the owner on the provider's page holds up only its own account: the
/// others' runs go on (owner, 0.12.5: the Outlook.es checks stopped while a Gmail page waited).
#[test]
fn other_accounts_keep_syncing_while_the_owner_signs_in() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let runs = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = runs.clone();
    app.services.sync = Some(Box::new(move |_: mailcraft_model::sync::SyncRequest| {
        counted.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = std::sync::mpsc::channel();
        std::mem::forget(tx);
        rx
    }));
    app.services.sign_in = Some(Box::new(|_| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::mem::forget(tx);
        rx
    }));
    let added = app
        .execute(
            "account.add",
            serde_json::json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example", "port": 993}, "outgoing": {"host": "smtp.mail.example", "port": 465}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.passwords.insert(added, "invented-password".into());
    let first = app.session.mailbox.accounts[0].clone();
    assert_ne!(first.id, added);
    app.begin_sign_in(first.id, Default::default(), &first.email, String::new(), false, false).unwrap();
    assert_eq!(app.account_troubles()[0].2, Trouble::Waiting);
    app.sync_queue.push_back((first.id, true));
    app.sync_queue.push_back((added, true));
    app.auto_sync();
    assert_eq!(runs.load(Ordering::SeqCst), 0, "not the account on the page");
    app.auto_sync();
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the other account's run went ahead");
}

/// The headless sign-in question gives the owner at least a minute (owner: "the admin broker
/// allows 1 minute at least").
#[test]
fn the_question_stays_up_long_enough() {
    let q = crate::sign_in_notice::question("one@example.org", Default::default());
    assert!(q.seconds >= 60);
    assert!(q.text.contains("one@example.org"));
}
