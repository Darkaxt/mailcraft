// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Saved virtual queries preserve exact scopes, owner state and native persistence.

use mailcraft_engine::{Scope, Session};
use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::{Value, json};

fn fixture() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email":"first@example.org"})).unwrap()["id"].as_u64().unwrap();
    let b = s.run("account.add", &json!({"email":"second@example.org"})).unwrap()["id"].as_u64().unwrap();
    (s, a, b)
}
fn message(s: &mut Session, account: Id, subject: &str) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let id = s
        .mailbox
        .add_message(Message {
            account,
            folder,
            subject: subject.into(),
            message_id: format!("fixture-{}@example.org", s.mailbox.messages.len()),
            from: Address::new("Billing", "billing@example.org"),
            body: Body { text: "A Café invoice, precisely!".into(), html: None },
            ..Default::default()
        })
        .unwrap();
    s.mailbox_changed();
    id
}
fn create(s: &mut Session, params: Value) -> Id {
    s.run("folder.newSearchFolder", &params).unwrap()["id"].as_u64().unwrap()
}
fn results(s: &mut Session) -> Vec<Id> {
    let mut ids: Vec<_> = s.rows().iter().map(|r| r.id).collect();
    ids.sort_unstable();
    ids
}

#[test]
fn saved_exact_case_query_reopens_and_tracks_arrivals_and_filing_without_provider_folders() {
    let (mut s, a, b) = fixture();
    let one = message(&mut s, a, "Invoice A");
    message(&mut s, a, "invoice a");
    let two = message(&mut s, b, "Invoice A");
    let folders = s.mailbox.folders.clone();
    let queued = s.mailbox.queued.clone();
    let id = create(&mut s, json!({"name":"Exact invoices","query":"subject:\"Invoice A\" case:sensitive","scope":"inboxes"}));
    assert_eq!(s.mailbox.folders, folders);
    assert_eq!(s.mailbox.queued, queued);
    s.run("searchFolders.select", &json!({"id":id})).unwrap();
    assert_eq!(results(&mut s), [one, two]);
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).unwrap();
    let mut s = Session::new(mailcraft_engine::io::read_mailbox(&bytes).unwrap());
    s.run("searchFolders.select", &json!({"id":id})).unwrap();
    assert_eq!(results(&mut s), [one, two]);
    let fresh = message(&mut s, b, "Invoice A");
    assert!(results(&mut s).contains(&fresh));
    let sent = s.mailbox.folder_of_kind(a, FolderKind::Sent).unwrap();
    s.run("mail.move", &json!({"ids":[one],"folder":sent})).unwrap();
    assert_eq!(results(&mut s), [two, fresh]);
}

#[test]
fn preview_and_inventory_leave_owner_query_selection_messages_and_provider_queues_unchanged() {
    let (mut s, a, b) = fixture();
    let one = message(&mut s, a, "Invoice A");
    message(&mut s, b, "Other mail");
    s.run("view.allInboxes", &json!({})).unwrap();
    s.run("mail.select", &json!({"id":one})).unwrap();
    let mailbox = s.mailbox.clone();
    let view = s.view.clone();
    let selected = s.sel.clone();
    let preview = s.run("searchFolders.preview", &json!({"name":"Literal phrase","query":"\"Café invoice, precisely!\"","scope":"inboxes"})).unwrap();
    assert_eq!(preview["total"], 2);
    assert!(preview.get("searchIndex").is_some());
    assert_eq!(s.mailbox, mailbox);
    assert_eq!(s.view, view);
    assert_eq!(s.sel, selected);
    let inventory = s.run("searchFolders.list", &json!({})).unwrap();
    assert_eq!(inventory["queriesExecuted"], 0);
    assert_eq!(s.mailbox, mailbox);
    assert_eq!(s.view, view);
}

#[test]
fn edit_remove_and_undo_affect_only_saved_definition_and_explicit_result_view() {
    let (mut s, a, _) = fixture();
    let id = message(&mut s, a, "Invoice A");
    let saved = create(&mut s, json!({"name":"Invoices","query":"subject:Invoice","scope":"inboxes"}));
    s.run("searchFolders.select", &json!({"id":saved})).unwrap();
    let messages = s.mailbox.messages.clone();
    let queues = (s.mailbox.pending.clone(), s.mailbox.queued.clone());
    s.run("searchFolders.edit", &json!({"id":saved,"name":"Not present","query":"subject:absent","scope":"inboxes"})).unwrap();
    assert!(results(&mut s).is_empty());
    assert_eq!(s.view.search_folder, Some(saved));
    s.run("edit.undo", &json!({})).unwrap();
    s.run("searchFolders.select", &json!({"id":saved})).unwrap();
    assert_eq!(results(&mut s), [id]);
    s.run("searchFolders.remove", &json!({"id":saved})).unwrap();
    assert!(s.mailbox.search_folders.is_empty());
    assert_eq!(s.mailbox.messages, messages);
    assert_eq!((s.mailbox.pending.clone(), s.mailbox.queued.clone()), queues);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.search_folders[0].id, saved);
}

#[test]
fn a_lost_scope_anchor_never_falls_back_to_another_accounts_inbox() {
    let (mut s, a, b) = fixture();
    message(&mut s, a, "Invoice A");
    let other = message(&mut s, b, "Invoice B");
    let anchor = s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap();
    let saved = create(&mut s, json!({"name":"Second mailbox","query":"subject:Invoice","scope":"mailbox","folder":anchor}));
    s.run("searchFolders.select", &json!({"id":saved})).unwrap();
    assert_eq!(results(&mut s), [other]);
    s.mailbox.folders.retain(|folder| folder.id != anchor);
    s.mailbox_changed();
    assert_eq!(s.view.folder, anchor);
    assert!(results(&mut s).is_empty());
    let report = s.run("mail.list", &json!({})).unwrap();
    assert!(!report["scopeProblem"].is_null());
    assert!(s.run("searchFolders.select", &json!({"id":saved})).is_err());
    assert!(s.run("view.search", &json!({"query":"subject:Invoice"})).is_err());
    let available = s.run("searchFolders.list", &json!({})).unwrap();
    assert_eq!(available["searchFolders"][0]["available"], false);
    let first = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    s.run("folder.select", &json!({"id":first})).unwrap();
    assert_eq!(s.view.search_folder, None);
}

#[test]
fn alias_saved_filter_is_exact_and_does_not_merge_account_boundaries() {
    let (mut s, a, b) = fixture();
    let one = message(&mut s, a, "Alias mail");
    let two = message(&mut s, b, "Other alias");
    for (id, alias) in [(one, "studio@example.org"), (two, "another@example.org")] {
        s.mailbox.message_mut(id).unwrap().headers = vec![
            ("X-SimpleLogin-Type".into(), "Forward".into()),
            ("X-SimpleLogin-Envelope-To".into(), alias.into()),
            ("Delivered-To".into(), alias.into()),
            ("Received".into(), "from mail-200.simplelogin.co (mail-200.simplelogin.co [192.0.2.10]) by mx.example.org".into()),
            ("X-SimpleLogin-Original-From".into(), "sender@example.org".into()),
        ];
    }
    let saved = create(&mut s, json!({"name":"Studio alias","query":"alias:studio@example.org","scope":"inboxes"}));
    s.run("searchFolders.select", &json!({"id":saved})).unwrap();
    assert_eq!(s.view.scope, Scope::Inboxes);
    assert_eq!(results(&mut s), [one]);
    let before = s.mailbox.clone();
    for params in [
        json!({"name":"bad","query":"\"unterminated","scope":"everywhere"}),
        json!({"name":"bad","query":"invoice","scope":"folder","folder":u64::MAX}),
        json!({"name":"bad","query":"invoice","scope":"not-real"}),
    ] {
        assert!(s.run("folder.newSearchFolder", &params).is_err());
        assert_eq!(s.mailbox, before);
    }
}
