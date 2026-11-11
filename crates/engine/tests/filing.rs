// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Filing is reviewed before mutation and preserves unrelated rules and message identity.

use mailcraft_engine::{Scope, Session};
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult};
use mailcraft_model::{Action, Address, Body, Condition, FolderKind, Id, Message};
use serde_json::{Value, json};

fn setup() -> (Session, [Id; 3], Id, Id) {
    let mut s = Session::empty();
    let ms = s.run("account.add", &json!({"email":"owner@outlook.example","kind":"microsoft"})).unwrap()["id"].as_u64().unwrap();
    mailcraft_engine::cmd::graph_mail::apply_graph_mail(
        &mut s,
        GraphMailResult {
            account: ms,
            email: "owner@outlook.example".into(),
            listing: Some(FolderListing {
                folders: vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }],
                complete: true,
            }),
            ..Default::default()
        },
    )
    .unwrap();
    let a = s.run("account.add", &json!({"email":"first@example.org"})).unwrap()["id"].as_u64().unwrap();
    let b = s.run("account.add", &json!({"email":"second@example.org"})).unwrap()["id"].as_u64().unwrap();
    let parent = s.run("folder.new", &json!({"account":ms,"parent":null,"name":"Shopping"})).unwrap()["id"].as_u64().unwrap();
    let new = s.run("folder.new", &json!({"parent":parent,"name":"StudioOffice"})).unwrap()["id"].as_u64().unwrap();
    let old = s.run("folder.new", &json!({"parent":parent,"name":"Old company"})).unwrap()["id"].as_u64().unwrap();
    (s, [ms, a, b], new, old)
}

fn mail(s: &mut Session, account: Id, sender: &str) -> Id {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox
        .add_message(Message {
            account,
            folder,
            from: Address::new("Display is not the predicate", sender),
            message_id: format!("fixture-{}@example.org", s.mailbox.messages.len()),
            subject: "Invoice".into(),
            body: Body { text: "Complete offline text".into(), html: None },
            ..Default::default()
        })
        .unwrap()
}

fn add_rule(s: &mut Session, folder: Id, senders: &[&str]) -> Id {
    s.run(
        "rules.add",
        &json!({"name":"Company filing","server":false,"conditions":[{"type":"fromAddresses","value":senders}],
        "actions":[{"type":"moveTo","value":folder}]}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap()
}

fn review(s: &mut Session, ids: &[Id], folder: Id) -> Value {
    s.run("filing.preview", &json!({"ids":ids,"folder":folder})).unwrap()
}
fn apply(s: &mut Session, ids: &[Id], folder: Id, report: &Value, choice: &str) -> Value {
    s.run("filing.apply", &json!({"ids":ids,"folder":folder,"review":report["review"],"choice":choice})).unwrap()
}

#[test]
fn auto_files_multiple_accounts_and_consolidates_future_senders_without_copying_identities() {
    let (mut s, a, new, _) = setup();
    let one = mail(&mut s, a[1], "billing@example.org");
    let two = mail(&mut s, a[2], "receipts@example.org");
    let report = review(&mut s, &[one, two], new);
    assert_eq!(report["conflicts"], json!([]));
    apply(&mut s, &[one, two], new, &report, "auto");
    assert_eq!(s.mailbox.message(one).unwrap().origin_account(), a[1]);
    assert_eq!(s.mailbox.message(two).unwrap().origin_account(), a[2]);
    assert_eq!(s.mailbox.message(one).unwrap().account, a[0]);
    assert_eq!(s.mailbox.message(one).unwrap().folder, new);
    assert_eq!(s.mailbox.rules.len(), 1);
    assert_eq!(s.mailbox.rules[0].account, 0);
    assert_eq!(s.mailbox.messages.len(), 2);
    let future = mail(&mut s, a[2], "billing@example.org");
    s.mailbox.apply_rules(future);
    assert_eq!(s.mailbox.message(future).unwrap().folder, new);
    assert!(!s.mailbox.queued.is_empty(), "server placement is pending, not falsely confirmed");
}

#[test]
fn reroute_splits_only_selected_alternatives_and_received_history_preserving_other_actions() {
    let (mut s, a, new, old) = setup();
    let rule = add_rule(&mut s, old, &["billing@example.org", "other@example.org"]);
    s.mailbox.rules.iter_mut().find(|r| r.id == rule).unwrap().actions.push(Action::MarkRead);
    let selected = mail(&mut s, a[1], "billing@example.org");
    let history = mail(&mut s, a[0], "billing@example.org");
    let unrelated = mail(&mut s, a[2], "other@example.org");
    s.mailbox.move_message(history, old).unwrap();
    let report = review(&mut s, &[selected], new);
    assert_eq!(report["conflicts"][0]["id"], rule);
    assert_eq!(report["historicalMessages"], 2);
    apply(&mut s, &[selected], new, &report, "reroute");
    assert_eq!(s.mailbox.message(history).unwrap().folder, new);
    assert_ne!(s.mailbox.message(unrelated).unwrap().folder, new);
    let original = s.mailbox.rules.iter().find(|r| r.id == rule).unwrap();
    assert!(original.actions.contains(&Action::MoveTo(old)) && original.actions.contains(&Action::MarkRead));
    assert!(original.conditions.iter().any(|c| matches!(c, Condition::Unless(_))));
    s.mailbox.apply_rules(unrelated);
    assert_eq!(s.mailbox.message(unrelated).unwrap().folder, old);
    assert!(s.mailbox.message(unrelated).unwrap().read);
    let future = mail(&mut s, a[1], "billing@example.org");
    s.mailbox.apply_rules(future);
    assert_eq!(s.mailbox.message(future).unwrap().folder, new);
    assert!(s.mailbox.message(future).unwrap().read, "split sender retains the original non-filing action");
    assert_eq!(review(&mut s, &[future], new)["conflicts"], json!([]), "the exact remainder exception is disjoint");
}

#[test]
fn once_survives_native_reopen_and_repeated_rule_processing_without_exempting_future_mail() {
    let (mut s, a, new, old) = setup();
    add_rule(&mut s, old, &["billing@example.org"]);
    let selected = mail(&mut s, a[1], "billing@example.org");
    let report = review(&mut s, &[selected], new);
    let rules = s.mailbox.rules.clone();
    apply(&mut s, &[selected], new, &report, "once");
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).unwrap();
    let mut s = Session::new(mailcraft_engine::io::read_mailbox(&bytes).unwrap());
    for _ in 0..3 {
        s.mailbox.apply_rules(selected);
        s.mailbox.run_rules(selected, true);
    }
    assert_eq!(s.mailbox.message(selected).unwrap().folder, new);
    assert_eq!(s.mailbox.message(selected).unwrap().filing_override, Some(new));
    assert_eq!(s.mailbox.rules, rules);
    let future = mail(&mut s, a[2], "billing@example.org");
    s.mailbox.apply_rules(future);
    assert_eq!(s.mailbox.message(future).unwrap().folder, old);
}

#[test]
fn reuse_preserves_disabled_definitions_and_stale_review_cannot_mutate() {
    let (mut s, a, new, old) = setup();
    let rule = add_rule(&mut s, old, &["billing@example.org"]);
    s.run("rules.enable", &json!({"id":rule,"value":false})).unwrap();
    let id = mail(&mut s, a[1], "billing@example.org");
    let report = review(&mut s, &[id], new);
    assert_eq!(report["conflicts"][0]["enabled"], false);
    s.run("account.rowColor", &json!({"id":a[1],"color":"#EEDDF1"})).unwrap();
    let before = s.mailbox.clone();
    assert!(s.run("filing.apply", &json!({"ids":[id],"folder":new,"review":report["review"],"choice":"once"})).is_err());
    assert_eq!(s.mailbox, before);
    let report = review(&mut s, &[id], new);
    s.run("filing.apply", &json!({"ids":[id],"folder":new,"review":report["review"],"choice":"reuse","reuseFolder":old})).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().folder, old);
    assert!(!s.mailbox.rules.iter().find(|r| r.id == rule).unwrap().enabled);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().filing_override, None);
    assert_eq!(s.mailbox.message(id).unwrap().account, a[1]);
}

#[test]
fn unfiled_view_keeps_external_custom_folder_mail_visible_and_retains_real_tree_ids() {
    let (mut s, a, new, _) = setup();
    let external = s.mailbox.add_folder(a[1], None, "Unmatched").unwrap();
    let id = mail(&mut s, a[1], "new@example.org");
    s.mailbox.move_message(id, external).unwrap();
    let filed = mail(&mut s, a[2], "filed@example.org");
    s.mailbox.move_message(filed, new).unwrap();
    s.run("view.filing", &json!({"account":a[0]})).unwrap();
    let ids: Vec<_> = s.rows().iter().map(|r| r.id).collect();
    assert!(ids.contains(&id) && !ids.contains(&filed));
    s.run("view.filing", &json!({"account":a[0],"folder":new})).unwrap();
    assert_eq!(s.rows()[0].id, filed);
    s.run("view.allInboxes", &json!({})).unwrap();
    assert_eq!(s.view.filing, None);
    assert_eq!(s.view.scope, Scope::Inboxes);
}

#[test]
fn alias_origin_colour_is_retained_without_substituting_untrusted_original_sender() {
    let (mut s, a, new, _) = setup();
    let id = mail(&mut s, a[1], "reverse@simplelogin.example");
    s.mailbox.message_mut(id).unwrap().headers = vec![
        ("X-SimpleLogin-Type".into(), "Forward".into()),
        ("X-SimpleLogin-Envelope-To".into(), "studio@example.org".into()),
        ("X-SimpleLogin-Original-From".into(), "claimed@example.org".into()),
        ("Received".into(), "from mail-200.simplelogin.co (mail-200.simplelogin.co [192.0.2.10]) by mx.example.org".into()),
    ];
    s.mailbox.message_mut(id).unwrap().to = vec![Address::new("", "studio@example.org")];
    s.run("aliases.rowColor", &json!({"account":a[1],"alias":"studio@example.org","color":"#E5DDF3"})).unwrap();
    let report = review(&mut s, &[id], new);
    assert_eq!(report["senders"], json!(["reverse@simplelogin.example"]));
    apply(&mut s, &[id], new, &report, "auto");
    let message = s.mailbox.message(id).unwrap();
    assert_eq!(message.origin_alias().as_deref(), Some("studio@example.org"));
    let origin = s.mailbox.account(message.origin_account()).unwrap();
    assert_eq!(origin.origin_row_color(message.origin_alias().as_deref()).hex(), "#E5DDF3");
    assert_eq!(message.from.email, "reverse@simplelogin.example");
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).unwrap();
    let opened = mailcraft_engine::io::read_mailbox(&bytes).unwrap();
    let message = opened.message(id).unwrap();
    assert_eq!(message.origin_alias().as_deref(), Some("studio@example.org"));
    assert_eq!(opened.account(a[1]).unwrap().origin_row_color(message.origin_alias().as_deref()).hex(), "#E5DDF3");
    s.run("mail.reply", &json!({"id":id})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().account, a[1], "filing does not silently choose a different reply identity");
}

#[test]
fn combined_conversations_do_not_merge_copies_from_different_observed_accounts_after_filing() {
    let (mut s, a, new, _) = setup();
    let one = mail(&mut s, a[1], "billing@example.org");
    let two = mail(&mut s, a[2], "billing@example.org");
    s.mailbox.message_mut(one).unwrap().message_id = "shared-id@example.org".into();
    s.mailbox.message_mut(two).unwrap().message_id = "shared-id@example.org".into();
    let report = review(&mut s, &[one, two], new);
    apply(&mut s, &[one, two], new, &report, "auto");
    s.run("view.filing", &json!({"account":a[0],"folder":new})).unwrap();
    s.view.conversations = true;
    let rows = s.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.members.len() == 1 && r.others.is_empty()));
}

#[test]
fn partially_filed_thread_keeps_its_origin_links_and_scope_switch_invalidates_cached_conversation() {
    let (mut s, a, new, _) = setup();
    let root = mail(&mut s, a[1], "billing@example.org");
    let reply = mail(&mut s, a[1], "billing@example.org");
    let other = mail(&mut s, a[2], "billing@example.org");
    s.mailbox.message_mut(root).unwrap().message_id = "root@example.org".into();
    s.mailbox.message_mut(reply).unwrap().in_reply_to = "root@example.org".into();
    s.mailbox.message_mut(other).unwrap().message_id = "root@example.org".into();
    s.mailbox_changed();
    let report = review(&mut s, &[root], new);
    apply(&mut s, &[root], new, &report, "once");
    s.run("view.filing", &json!({"account":a[0],"folder":new})).unwrap();
    s.view.conversations = true;
    let row = s.rows().first().cloned().unwrap();
    assert_eq!(row.members, [root]);
    assert_eq!(row.others, [reply]);
    let mut thread = s.conversation(root).as_ref().clone();
    thread.sort_unstable();
    assert_eq!(thread, [root, reply]);
    assert!(!thread.contains(&other));
    s.run("folder.select", &json!({"id":new})).unwrap();
    assert_eq!(s.conversation(root).as_slice(), [root], "a physical folder uses current-account conversation ownership");
    s.run("view.allInboxes", &json!({})).unwrap();
    let mut thread = s.conversation(reply).as_ref().clone();
    thread.sort_unstable();
    assert_eq!(thread, [root, reply]);
}

#[test]
fn removed_receiving_account_cannot_silently_change_a_filed_messages_reply_sender() {
    let (mut s, a, new, _) = setup();
    let id = mail(&mut s, a[1], "billing@example.org");
    let report = review(&mut s, &[id], new);
    apply(&mut s, &[id], new, &report, "once");
    s.mailbox.remove_account(a[1]).unwrap();
    s.mailbox_changed();
    assert!(s.mailbox.message(id).is_some(), "filed content belongs to the canonical account");
    assert!(s.run("mail.reply", &json!({"id":id})).is_err());
    assert!(s.compose.is_none());
}

#[test]
fn server_owned_or_deleting_definitions_are_visible_but_never_rewritten() {
    for server_owned in [false, true] {
        let (mut s, a, new, old) = setup();
        let rule = add_rule(&mut s, old, &["billing@example.org"]);
        let original = s.mailbox.rules.iter_mut().find(|r| r.id == rule).unwrap();
        if server_owned {
            original.keep = mailcraft_model::Keep::Server;
        } else {
            original.actions.push(Action::Delete);
        }
        let id = mail(&mut s, a[1], "billing@example.org");
        let report = review(&mut s, &[id], new);
        assert_eq!(report["rerouteAllowed"], false);
        let before = s.mailbox.clone();
        assert!(s.run("filing.apply", &json!({"ids":[id],"folder":new,"review":report["review"],"choice":"reroute"})).is_err());
        assert_eq!(s.mailbox, before);
        apply(&mut s, &[id], new, &report, "once");
        assert_eq!(s.mailbox.rules, before.rules);
    }
}

#[test]
fn incomplete_selected_content_rolls_back_the_entire_rule_and_move_transaction() {
    let (mut s, a, new, _) = setup();
    let good = mail(&mut s, a[1], "billing@example.org");
    let unavailable = mail(&mut s, a[2], "receipts@example.org");
    s.mailbox.message_mut(unavailable).unwrap().incomplete = true;
    let report = review(&mut s, &[good, unavailable], new);
    assert_eq!(report["selectedUnavailable"], 1);
    let before = s.mailbox.clone();
    assert!(s.run("filing.apply", &json!({"ids":[good,unavailable],"folder":new,"review":report["review"]})).is_err());
    assert_eq!(s.mailbox, before, "no partial sender rule, move, placement or origin marker survives failure");
}
