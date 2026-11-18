// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The local archive (v0.12.10): a Microsoft account's old mail moves into twins of its folders
//! on this computer, shows in those folders as before, and leaves the server for good only once
//! it is whole, saved and backed up.

use mailcraft_model::sync::{MessageChange, ServerMessage};
use mailcraft_model::{FolderKind, Id, Message, ServerFolder};
use serde_json::json;

use super::archive::{self, cutoff};
use super::graph_up::message_changes;
use super::graph_up::tests::{NOW, setup};
use crate::Session;
use crate::agent::{self, Access, Class};

/// 2020-03-01, long before [`NOW`] (2027-01-15).
const OLD: i64 = 1_583_020_800;

/// [`setup`] with its message old, an old message in a folder under the Inbox, an old one in
/// Deleted Items, an old draft and a recent one: the session, the account, the two old ones that
/// are to be archived, and the folder under the Inbox.
fn mailbox() -> (Session, Id, Id, Id, Id) {
    let (mut s, account, m, _) = setup();
    {
        let first = s.mailbox.message_mut(m).unwrap();
        (first.date, first.received) = (OLD, OLD);
        first.server.as_mut().unwrap().size = 4000;
    }
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let bills = s.mailbox.add_folder(account, Some(inbox), "Bills").unwrap();
    s.mailbox.folder_mut(bills).unwrap().server = Some(ServerFolder { id: "F-bi".into(), name: "Bills".into(), ..Default::default() });
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let mut add = |folder: Id, id: &str, when: i64, draft: bool| {
        let on = match folder {
            f if f == bills => "F-bi",
            f if f == deleted => "F-de",
            _ => "F-in",
        };
        let server = ServerMessage { id: id.into(), folder: on.into(), size: 1000, ..Default::default() };
        s.mailbox
            .add_message(Message { folder, subject: id.into(), date: when, received: when, draft, server: Some(server), ..Default::default() })
            .unwrap()
    };
    let bill = add(bills, "M-2", OLD + 86_400, false);
    add(deleted, "M-3", OLD, false);
    add(inbox, "M-4", OLD, true);
    add(inbox, "M-5", NOW - 86_400 * 30, false);
    (s, account, m, bill, bills)
}

/// A folder whose old mail is kept on this computer isn't deleted while that mail is there: it
/// would be left with no folder to show it in (v0.12.13).
#[test]
fn a_folder_with_archived_mail_is_not_deleted() {
    let (mut s, account, _, bill, bills) = mailbox();
    s.run("archive.set", &json!({"account": account, "years": 3})).unwrap();
    s.run("archive.step", &json!({})).unwrap();
    let why = s.run("folder.delete", &json!({"id": bills})).unwrap_err().to_string();
    assert!(why.contains("1 messages of this folder are kept on this computer"), "{why}");
    assert!(s.mailbox.folder(bills).is_some() && s.mailbox.message(bill).is_some());
}

#[test]
fn the_cutoff_is_the_same_day_years_before() {
    // 2027-01-15 → 2024-01-15.
    assert_eq!(cutoff(NOW, 3), 1_705_276_800);
}

#[test]
fn a_preview_counts_the_old_mail_that_would_move() {
    let (mut s, account, _, _, _) = mailbox();
    let p = s.run("archive.preview", &json!({"account": account, "years": 3})).unwrap();
    assert_eq!((p["count"].clone(), p["bytes"].clone()), (json!(2), json!(5000)));
    // Only a Microsoft account; a number of years.
    let local = s.mailbox.accounts.iter().find(|a| a.kind == mailcraft_model::AccountKind::Local).unwrap().id;
    assert!(s.run("archive.preview", &json!({"account": local, "years": 3})).is_err());
    assert!(s.run("archive.set", &json!({"account": account, "years": 0})).is_err());
}

#[test]
fn old_mail_moves_into_twins_of_its_folders_and_shows_in_them() {
    let (mut s, account, m, bill, bills) = mailbox();
    // Off: nothing moves.
    assert_eq!(s.run("archive.step", &json!({})).unwrap()["moved"], json!(0));
    s.run("archive.set", &json!({"account": account, "years": 3})).unwrap();
    assert_eq!(s.run("archive.step", &json!({})).unwrap()["moved"], json!(2));
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let twin_of = |id: Id| s.mailbox.folders.iter().find(|f| f.twin_of == Some(id)).unwrap().clone();
    let (top, inbox_twin, bills_twin) = (twin_of(account), twin_of(inbox), twin_of(bills));
    assert_eq!(top.name, "owner@example.org archive");
    assert_eq!((inbox_twin.parent, bills_twin.parent), (Some(top.id), Some(inbox_twin.id)));
    let local = top.account;
    assert!(s.mailbox.account(local).is_some_and(|a| a.kind == mailcraft_model::AccountKind::Local));
    for (id, folder) in [(m, inbox_twin.id), (bill, bills_twin.id)] {
        let here = s.mailbox.message(id).unwrap();
        assert_eq!((here.folder, here.account), (folder, local));
        assert!(here.moving_out.as_ref().is_some_and(|out| out.archive && out.account == account));
    }
    // The Inbox's list shows its archived mail; the folder under it, its own.
    s.view.folder = inbox;
    assert!(crate::list::in_view(&s)(s.mailbox.message(m).unwrap()));
    assert!(!crate::list::in_view(&s)(s.mailbox.message(bill).unwrap()));
    s.view.folder = bills;
    assert!(crate::list::in_view(&s)(s.mailbox.message(bill).unwrap()));
    // Nothing more to move; the status says what waits.
    assert_eq!(s.run("archive.step", &json!({})).unwrap()["moved"], json!(0));
    let status = s.run("archive.status", &json!({})).unwrap();
    let row = &status["accounts"][0];
    assert_eq!((row["years"].clone(), row["toMove"].clone(), row["waiting"].clone()), (json!(3), json!(0), json!(2)));
}

#[test]
fn archived_mail_leaves_the_server_for_good_only_once_backed_up() {
    let (mut s, account, m, bill, _) = mailbox();
    s.run("archive.set", &json!({"account": account, "years": 3})).unwrap();
    s.run("archive.step", &json!({})).unwrap();
    s.note_saved();
    assert!(archive::waits_for_backup(&s));
    assert_eq!(message_changes(&s, account, 100).0, []);
    s.note_backed_up([m]);
    assert_eq!(message_changes(&s, account, 100).0, [MessageChange::MovedOut { message: m, id: "M-1".into(), purge: true }]);
    s.note_backed_up([bill]);
    assert!(!archive::waits_for_backup(&s));
    assert_eq!(message_changes(&s, account, 100).0.len(), 2);
}

#[test]
fn archived_sent_mail_stays_out_of_all_mail() {
    let (mut s, account, _, _, _) = mailbox();
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let (sent_twin, inbox_twin) = (archive::twin(&mut s.mailbox, sent).unwrap(), archive::twin(&mut s.mailbox, inbox).unwrap());
    assert!(!super::filing::receiving_folder(&s, sent_twin));
    assert!(super::filing::receiving_folder(&s, inbox_twin));
    // The same twin each time.
    assert_eq!(archive::twin(&mut s.mailbox, sent).unwrap(), sent_twin);
}

#[test]
fn an_agent_needs_the_owner_to_turn_it_on() {
    let (s, account, _, _, _) = mailbox();
    assert_eq!(agent::classify(&s, "archive.set", &json!({"account": account, "years": 3})), Access::Owner(Class::Delete));
    assert_eq!(agent::classify(&s, "archive.set", &json!({"account": account, "years": null})), Access::Safe);
    for id in ["archive.preview", "archive.status", "archive.step"] {
        assert_eq!(agent::access(id), Some(Access::Safe), "{id}");
    }
}

/// The owner's yes reaches turning it on while the mailbox syncs: new mail arriving changes
/// nothing the question named. More old mail does: its count is in the question.
#[test]
fn the_owners_yes_holds_while_new_mail_arrives() {
    let (mut s, account, _, _, _) = mailbox();
    let params = json!({"account": account, "years": 3});
    let shown = agent::bind::Shown::new(&s, "archive.set", &params, &agent::describe(&s, "archive.set", &params)).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let arrive = |s: &mut Session, id: &str, when: i64| {
        let server = ServerMessage { id: id.into(), folder: "F-in".into(), size: 1000, ..Default::default() };
        s.mailbox
            .add_message(Message { folder: inbox, subject: id.into(), date: when, received: when, server: Some(server), ..Default::default() })
            .unwrap();
        s.mailbox_changed();
    };
    arrive(&mut s, "M-new", NOW - 3600);
    assert!(shown.check(&s, "archive.set", &params).is_ok());
    arrive(&mut s, "M-old", OLD);
    assert!(shown.check(&s, "archive.set", &params).is_err());
}
