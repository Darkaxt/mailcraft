// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::{Action, Condition, FolderKind, Id, Keep, Mailbox, Message, Rule, RuleGone, ServerFolder, ServerRule};
use serde_json::json;

use crate::Session;

fn definition(id: Id, account: Id, actions: Vec<Action>) -> Rule {
    Rule {
        id,
        account,
        name: format!("Rule {id}"),
        enabled: true,
        conditions: vec![Condition::SubjectContains("invoice".into())],
        actions,
        ..Default::default()
    }
}

#[test]
fn rules_lifecycle_delete_subtree_removes_whole_cross_account_move_and_copy_definitions_and_undoes_together() {
    let mut s = Session::at(Mailbox::with_local_account("", "owner@example.org"), 0, 0);
    let account = s.mailbox.accounts[0].id;
    let source = s.run("account.add", &json!({"email":"source@example.org"})).unwrap()["id"].as_u64().unwrap();
    let parent = s.mailbox.add_folder(account, None, "Company").unwrap();
    let child = s.mailbox.add_folder(account, Some(parent), "Receipts").unwrap();
    let trash = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let mail = s.mailbox.add_message(Message { folder: child, subject: "Keep the message".into(), ..Default::default() }).unwrap();
    s.mailbox.rules = vec![
        definition(900, source, vec![Action::MoveTo(parent)]),
        definition(901, 0, vec![Action::CopyTo(child), Action::MarkRead]),
        definition(902, source, vec![Action::Delete]),
        definition(903, source, vec![Action::MoveTo(trash)]),
        definition(904, source, vec![Action::MoveTo(999_999)]),
    ];
    let originals = s.mailbox.rules.clone();
    let message = s.mailbox.message(mail).unwrap().clone();
    let reply = s.run("folder.delete", &json!({"id":parent})).unwrap();
    assert_eq!(reply["permanently"], false);
    assert_eq!(s.mailbox.rules.iter().map(|rule| rule.id).collect::<Vec<_>>(), [902, 903, 904]);
    assert_eq!(s.mailbox.message(mail), Some(&message));
    let persisted: Mailbox = serde_json::from_slice(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    assert_eq!(persisted.rules, s.mailbox.rules);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.rules, originals);
    assert_eq!(s.mailbox.folder(parent).unwrap().parent, None);
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.rules.iter().map(|rule| rule.id).collect::<Vec<_>>(), [902, 903, 904]);
    s.run("folder.delete", &json!({"id":parent})).unwrap();
    assert!(s.mailbox.folder(child).is_none());
}

#[test]
fn rules_lifecycle_move_only_into_deleted_items_removes_descendant_redirects() {
    let mut mb = Mailbox::with_local_account("", "owner@example.org");
    let account = mb.accounts[0].id;
    let parent = mb.add_folder(account, None, "Company").unwrap();
    let child = mb.add_folder(account, Some(parent), "Receipts").unwrap();
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    let trash = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let nested_trash = mb.add_folder(account, Some(trash), "Earlier deletion").unwrap();
    mb.rules.push(definition(900, account, vec![Action::MoveTo(child)]));
    assert!(mb.move_folder(parent, Some(child)).is_err());
    mb.move_folder(parent, Some(archive)).unwrap();
    assert_eq!(mb.rules.len(), 1);
    mb.move_folder(parent, Some(nested_trash)).unwrap();
    assert!(mb.rules.is_empty());
}

#[test]
fn rules_lifecycle_provider_deletion_intent_survives_large_queues_refusals_restart_and_old_downloads() {
    use super::rules_remote::{apply_rule_outcomes, merge_rules, rule_upload};
    use mailcraft_model::remote_rules::{RemoteRule, RuleOpKind, RuleOutcome, RuleResult, RulesDownload, RulesUploaded};
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org","kind":"microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folder = s.mailbox.add_folder(account, None, "Company").unwrap();
    let mut rule = definition(900, account, vec![Action::MoveTo(folder)]);
    rule.keep = Keep::Server;
    rule.server = Some(ServerRule { id: "remote-rule".into(), ..Default::default() });
    s.mailbox.rules.push(rule);
    s.mailbox.rule_gone = (0..501).map(|n| RuleGone { account, id: format!("old-{n}"), ..Default::default() }).collect();
    s.run("folder.delete", &json!({"id":folder})).unwrap();
    assert!(s.mailbox.rules.is_empty());
    assert!(s.mailbox.rule_gone.iter().any(|gone| gone.id == "remote-rule"));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.rules.len(), 1);
    assert_eq!(s.mailbox.rule_gone.len(), 501);
    s.run("edit.redo", &json!({})).unwrap();
    let downloaded = RemoteRule { id: "remote-rule".into(), name: "Original".into(), move_to: "company".into(), ..Default::default() };
    let mut download = RulesDownload { account, email: "owner@example.org".into(), complete: false, rules: vec![downloaded] };
    merge_rules(&mut s.mailbox, &download).unwrap();
    assert!(s.mailbox.rules.is_empty(), "an old response must not resurrect the definition");
    let outcome = RulesUploaded {
        account,
        email: "owner@example.org".into(),
        outcomes: vec![RuleOutcome {
            kind: RuleOpKind::Delete,
            id: "remote-rule".into(),
            result: RuleResult::Refused,
            error: "read only".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let merged = apply_rule_outcomes(&mut s.mailbox, &outcome).unwrap();
    assert_eq!(merged.removed, 0);
    assert!(s.mailbox.rule_gone.iter().any(|gone| gone.id == "remote-rule" && gone.blocked == "read only"));
    let restored: Mailbox = serde_json::from_slice(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    assert!(!rule_upload(&restored, account).unwrap().ops.iter().any(|op| op.id == "remote-rule"));
    let listing = s.run("rules.list", &json!({"account":account})).unwrap();
    assert!(listing["pendingDeletions"].as_array().unwrap().iter().any(|gone| gone["blocked"] == "read only"));
    download.complete = true;
    merge_rules(&mut s.mailbox, &download).unwrap();
    assert_eq!(s.mailbox.rule_gone.len(), 1, "only the observed still-existing rule remains unresolved");
    download.rules.clear();
    merge_rules(&mut s.mailbox, &download).unwrap();
    assert!(s.mailbox.rule_gone.is_empty(), "a full authoritative read can confirm absence");
}

#[test]
fn rules_lifecycle_permanent_server_folder_deletion_is_not_resurrected_by_undo() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org","kind":"microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folder = s.mailbox.add_folder(account, None, "Company").unwrap();
    s.mailbox.folder_mut(folder).unwrap().server = Some(ServerFolder { id: "company".into(), ..Default::default() });
    s.mailbox.rules.push(definition(900, account, vec![Action::MoveTo(folder)]));
    s.run("folder.delete", &json!({"id":folder})).unwrap();
    s.run("folder.delete", &json!({"id":folder})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.folder(folder).is_none());
    assert!(s.mailbox.rules.is_empty(), "irreversible folder deletion cannot restore a dangling redirect");
}

#[test]
fn rules_lifecycle_authoritative_absence_marks_the_receipt_dirty_without_replaying_it_on_undo() {
    use super::rules_remote::apply_rules;
    use mailcraft_model::remote_rules::RulesDownload;
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email":"owner@example.org","kind":"microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.rule_gone.push(RuleGone { account, id: "already-removed".into(), ..Default::default() });
    s.run("folder.new", &json!({"name":"Unrelated folder","account":account,"parent":null})).unwrap();
    s.dirty = false;
    let reply = apply_rules(&mut s, &RulesDownload { account, email: "owner@example.org".into(), complete: true, ..Default::default() }).unwrap();
    assert!(reply.changed());
    assert!(s.dirty, "the cleared durable intent must be saved even without a live definition change");
    assert!(!s.can_undo(), "a provider-confirmed rule deletion is not reversible by restoring its local record");
    assert!(s.run("edit.undo", &json!({})).is_err());
    assert!(s.mailbox.rule_gone.is_empty());
}

#[test]
fn rules_lifecycle_undo_keeps_rules_and_intents_arriving_after_the_folder_change() {
    let mut s = Session::at(Mailbox::with_local_account("", "owner@example.org"), 0, 0);
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.add_folder(account, None, "Company").unwrap();
    let mut original = definition(900, account, vec![Action::MoveTo(folder)]);
    original.server = Some(ServerRule { id: "first-rule".into(), ..Default::default() });
    s.mailbox.rules.push(original.clone());
    s.run("folder.delete", &json!({"id":folder})).unwrap();
    let other = definition(901, account, vec![Action::MarkRead]);
    s.mailbox.rules.push(other.clone());
    let unrelated = RuleGone { account, id: "different-rule".into(), ..Default::default() };
    s.mailbox.rule_gone.push(unrelated.clone());
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.rules, [original, other]);
    assert_eq!(s.mailbox.rule_gone, [unrelated]);
}
