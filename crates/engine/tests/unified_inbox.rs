// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The virtual inbox keeps exact message/account identities across filing, searching and replies.

use mailcraft_engine::{Scope, Session};
use mailcraft_model::{Address, Body, FolderKind, Id, Message, Rgb};
use serde_json::{Value, json};

fn three() -> (Session, [Id; 3]) {
    let mut s = Session::empty();
    let mut add = |email: &str| s.run("account.add", &json!({"email":email,"kind":"local"})).unwrap()["id"].as_u64().unwrap();
    let accounts = [add("one@example.org"), add("two@example.org"), add("three@example.org")];
    (s, accounts)
}

fn message(s: &mut Session, account: Id, kind: FolderKind, subject: &str, read: bool) -> Id {
    let folder = s.mailbox.folder_of_kind(account, kind).unwrap();
    let id = s
        .mailbox
        .add_message(Message {
            account,
            folder,
            subject: subject.into(),
            read,
            message_id: format!("{account}-{}@example.org", subject.replace(' ', "-")),
            from: Address::new("Sender", "sender@example.org"),
            to: vec![Address::new("", s.mailbox.account(account).unwrap().email.clone())],
            body: Body { text: "offline body".into(), html: None },
            date: 100,
            received: 100,
            ..Default::default()
        })
        .unwrap();
    s.mailbox_changed();
    id
}

fn ids(result: &Value) -> Vec<Id> {
    result["messages"].as_array().unwrap().iter().map(|m| m["id"].as_u64().unwrap()).collect()
}

#[test]
fn union_tracks_real_inboxes_and_does_not_file_copy_or_merge_account_identities() {
    let (mut s, accounts) = three();
    let a = message(&mut s, accounts[0], FolderKind::Inbox, "Invoice A", false);
    let b = message(&mut s, accounts[1], FolderKind::Inbox, "Invoice B", true);
    let c = message(&mut s, accounts[2], FolderKind::Inbox, "Invoice C", false);
    message(&mut s, accounts[0], FolderKind::Sent, "Sent", true);
    message(&mut s, accounts[1], FolderKind::Junk, "Junk", false);
    let before = s.mailbox.messages.len();
    s.run("view.allInboxes", &json!({})).unwrap();
    assert_eq!(s.view.scope, Scope::Inboxes);
    let listed = s.run("mail.list", &json!({})).unwrap();
    assert_eq!(listed["total"], 3);
    for id in [a, b, c] {
        assert!(ids(&listed).contains(&id));
    }
    assert_eq!(s.mailbox.messages.len(), before);
    s.run("view.filter", &json!({"value":"unread"})).unwrap();
    assert_eq!(s.run("mail.list", &json!({})).unwrap()["total"], 2);
    s.run("view.filter", &json!({"value":"all"})).unwrap();
    let d = message(&mut s, accounts[1], FolderKind::Inbox, "New arrival", false);
    assert!(ids(&s.run("mail.list", &json!({})).unwrap()).contains(&d));
    let destination = s.mailbox.folder_of_kind(accounts[1], FolderKind::Sent).unwrap();
    s.run("mail.move", &json!({"ids":[b],"folder":destination})).unwrap();
    assert!(!ids(&s.run("mail.list", &json!({})).unwrap()).contains(&b));
}

#[test]
fn exact_search_clear_and_read_only_listing_preserve_the_owner_view() {
    let (mut s, a) = three();
    let id = message(&mut s, a[1], FolderKind::Inbox, "Invoice A", false);
    message(&mut s, a[0], FolderKind::Inbox, "invoice a", false);
    s.run("view.allInboxes", &json!({})).unwrap();
    s.run("view.search", &json!({"query":"subject:\"Invoice A\" case:sensitive"})).unwrap();
    let answer = s.run("mail.list", &json!({})).unwrap();
    assert_eq!(ids(&answer), [id]);
    s.run("view.clearSearch", &json!({})).unwrap();
    assert_eq!(s.view.scope, Scope::Inboxes);
    let view = serde_json::to_value(&s.view).unwrap();
    let selection = serde_json::to_value(&s.sel).unwrap();
    assert_eq!(s.run("mail.list", &json!({"scope":"inboxes","query":"subject:\"Invoice A\" case:sensitive"})).unwrap()["total"], 1);
    assert_eq!(serde_json::to_value(&s.view).unwrap(), view);
    assert_eq!(serde_json::to_value(&s.sel).unwrap(), selection);
    let folder = s.mailbox.folder_of_kind(a[1], FolderKind::Inbox).unwrap();
    s.run("folder.select", &json!({"id":folder})).unwrap();
    assert_eq!(s.view.scope, Scope::Folder);
    assert_eq!(s.run("mail.list", &json!({})).unwrap()["total"], 1);
}

#[test]
fn replies_follow_selected_message_accounts_and_colors_survive_native_save_reopen() {
    let (mut s, accounts) = three();
    let id = message(&mut s, accounts[2], FolderKind::Inbox, "Account isolation", false);
    s.run("view.allInboxes", &json!({})).unwrap();
    s.run("mail.reply", &json!({"id":id})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().account, accounts[2]);
    let default = s.mailbox.account(accounts[1]).unwrap().effective_row_color();
    s.run("account.rowColor", &json!({"id":accounts[1],"color":"#E8D4FA"})).unwrap();
    assert_eq!(s.mailbox.account(accounts[1]).unwrap().row_color, Some(Rgb(232, 212, 250)));
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).unwrap();
    let reopened = mailcraft_engine::io::read_mailbox(&bytes).unwrap();
    assert_eq!(reopened.account(accounts[1]).unwrap().row_color, Some(Rgb(232, 212, 250)));
    assert_eq!(reopened.messages.len(), s.mailbox.messages.len());
    s.run("account.rowColor", &json!({"id":accounts[1],"color":null})).unwrap();
    assert_eq!(s.mailbox.account(accounts[1]).unwrap().effective_row_color(), default);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.account(accounts[1]).unwrap().row_color, Some(Rgb(232, 212, 250)));
}

#[test]
fn alias_custom_colours_override_only_their_account_and_reset_to_inheritance() {
    let (mut s, accounts) = three();
    let account = accounts[0];
    s.run("account.rowColor", &json!({"id":account,"color":"#E8D4FA"})).unwrap();
    s.run("aliases.rowColor", &json!({"account":account,"alias":"Studio@Example.org","color":"#D5EEDC"})).unwrap();
    s.run("aliases.rowColor", &json!({"account":account,"alias":"shop@example.org","color":"#F0DFCD"})).unwrap();
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).unwrap();
    let mut reopened = Session::new(mailcraft_engine::io::read_mailbox(&bytes).unwrap());
    let a = reopened.mailbox.account(account).unwrap();
    assert_eq!(a.origin_row_color(Some("studio@example.org")), Rgb(213, 238, 220));
    assert_eq!(a.origin_row_color(Some("shop@example.org")), Rgb(240, 223, 205));
    assert_eq!(a.origin_row_color(Some("other@example.org")), Rgb(232, 212, 250));
    assert!(reopened.mailbox.account(accounts[1]).unwrap().alias_row_colors.is_empty());
    reopened.run("aliases.rowColor", &json!({"account":account,"alias":"STUDIO@example.org","color":null})).unwrap();
    assert_eq!(reopened.mailbox.account(account).unwrap().origin_row_color(Some("studio@example.org")), Rgb(232, 212, 250));
    assert_eq!(reopened.mailbox.account(account).unwrap().origin_row_color(Some("shop@example.org")), Rgb(240, 223, 205));
    reopened.run("edit.undo", &json!({})).unwrap();
    assert_eq!(reopened.mailbox.account(account).unwrap().origin_row_color(Some("studio@example.org")), Rgb(213, 238, 220));
    reopened.run("account.rowColor", &json!({"id":account,"color":null})).unwrap();
    assert_eq!(reopened.mailbox.account(account).unwrap().origin_row_color(Some("studio@example.org")), Rgb(213, 238, 220));
}
