// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages one after another from the same sender are one row (`view.senderGroups`, v0.12.37):
//! it shows the newest of them and how many there are, opens as a conversation's row does, and
//! Delete and Move on it closed take all of them.

use mailcraft_model::{Address, Id, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

/// Wednesday 2026-10-14 16:00 UTC.
fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A folder of five messages, newest first: three from one sender (one with its address in
/// capitals), one from another, and one more from the first; shown, grouped by sender.
fn runs() -> (Session, [Id; 5]) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.add_folder(account, None, "Runs").unwrap();
    let mut add = |from: &str, minutes: i64| {
        s.mailbox.add_message(Message { folder, from: Address::new("", from), date: now() - minutes * 60, ..Default::default() }).unwrap()
    };
    let ids = [add("a@x.com", 1), add("A@X.com", 2), add("a@x.com", 3), add("b@x.com", 4), add("a@x.com", 5)];
    run(&mut s, "folder.select", json!({"id": folder}));
    run(&mut s, "view.senderGroups", json!({"value": true}));
    (s, ids)
}

/// The rows as (message, members, under, open).
fn shape(s: &mut Session) -> Vec<(Id, Vec<Id>, Option<Id>, bool)> {
    s.rows().iter().map(|r| (r.id, r.members.clone(), r.under, r.open)).collect()
}

#[test]
fn messages_one_after_another_from_one_sender_are_one_row() {
    let (mut s, [a1, a2, a3, b, a4]) = runs();
    assert_eq!(
        shape(&mut s),
        [(a1, vec![a1, a2, a3], None, false), (b, vec![b], None, false), (a4, vec![a4], None, false)],
        "a run of three (an address in capitals is the same sender), then two rows of their own"
    );
    let rows = s.rows();
    let row = &rows[0];
    assert!(row.run && row.conversation() && row.size() == 3 && row.unread == 3);
    assert!(!rows[1].run && !rows[1].conversation());
}

#[test]
fn a_run_opens_and_closes_as_a_conversation_does() {
    let (mut s, [a1, a2, a3, b, a4]) = runs();
    let opened = run(&mut s, "mail.expandConversation", json!({"id": a1}));
    assert_eq!((opened["open"].clone(), opened["messages"].clone()), (json!(true), json!(3)));
    assert_eq!(
        shape(&mut s),
        [
            (a1, vec![a1, a2, a3], None, true),
            (a1, vec![a1], Some(a1), false),
            (a2, vec![a2], Some(a1), false),
            (a3, vec![a3], Some(a1), false),
            (b, vec![b], None, false),
            (a4, vec![a4], None, false),
        ]
    );
    // Newer mail from the sender joins it on top, and it stays open.
    let folder = s.mailbox.message(a1).unwrap().folder;
    let a0 = s.mailbox.add_message(Message { folder, from: Address::new("", "a@x.com"), date: now(), ..Default::default() }).unwrap();
    s.mailbox_changed();
    let rows = s.rows();
    assert_eq!((rows[0].id, rows[0].members.clone(), rows[0].open), (a0, vec![a0, a1, a2, a3], true));
    // Left closes it, from a message under it too.
    run(&mut s, "mail.select", json!({"id": a2}));
    let _ = s.rows();
    assert!(s.enabled("mail.collapseConversation") && !s.enabled("mail.expandConversation"));
    run(&mut s, "mail.collapseConversation", json!({}));
    assert_eq!(shape(&mut s)[0], (a0, vec![a0, a1, a2, a3], None, false));
}

#[test]
fn delete_on_a_closed_run_takes_all_of_it() {
    let (mut s, [a1, a2, a3, ..]) = runs();
    run(&mut s, "mail.select", json!({"id": a1}));
    let mut acting = s.acting_on();
    acting.sort_unstable();
    let mut all = vec![a1, a2, a3];
    all.sort_unstable();
    assert_eq!(acting, all);
}

#[test]
fn a_pinned_message_another_date_or_the_option_off_keeps_rows_apart() {
    let (mut s, [a1, a2, a3, ..]) = runs();
    run(&mut s, "mail.pin", json!({"ids": [a2]}));
    let rows = shape(&mut s);
    assert_eq!(rows[0].1, vec![a2], "pinned, on top and alone");
    assert_eq!(rows[1].1, vec![a1, a3], "the rest of the run, together again");

    // Yesterday's mail from the same sender is a row of its own under its own heading.
    let (mut s, [a1, _, _, b, _]) = runs();
    let folder = s.mailbox.message(a1).unwrap().folder;
    let old = s.mailbox.add_message(Message { folder, from: Address::new("", "b@x.com"), date: now() - 20 * 3600, ..Default::default() }).unwrap();
    s.mailbox_changed();
    let rows = shape(&mut s);
    assert!(rows.iter().any(|r| r.1 == vec![b]) && rows.iter().any(|r| r.1 == vec![old]), "{rows:?}");

    run(&mut s, "view.senderGroups", json!({"value": false}));
    assert!(s.rows().iter().all(|r| r.members.len() == 1 && !r.run));
    run(&mut s, "view.senderGroups", json!({"value": true}));
    run(&mut s, "view.conversations", json!({"value": true}));
    assert!(s.rows().iter().all(|r| !r.run), "conversation view groups by conversation");
}
