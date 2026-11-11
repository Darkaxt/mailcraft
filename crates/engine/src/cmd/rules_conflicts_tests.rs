// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Conflicts must be visible across folder editors and prevented before side effects.

use crate::{
    Session,
    agent::{Access, classify},
};
use mailcraft_model::{Address, Condition, FolderKind, Message};
use serde_json::{Value, json};

fn fixture() -> (Session, u64, u64, u64) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email":"source@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    (s, account, inbox, archive)
}

fn draft(account: u64, folder: u64) -> Value {
    json!({"name":"Company filing", "account":account, "server":false, "stop":true,
        "conditions":[{"type":"fromContains", "value":"billing@example.org"}], "actions":[{"type":"moveTo", "value":folder}]})
}

#[test]
fn conflicts_reject_add_edit_enable_and_from_message_without_changing_originals() {
    let (mut s, account, inbox, archive) = fixture();
    let first = s.run("rules.add", &draft(account, archive)).unwrap()["id"].as_u64().unwrap();
    let before = serde_json::to_value(&s.mailbox).unwrap();
    assert!(s.run("rules.add", &draft(account, inbox)).unwrap_err().to_string().contains("filing conflict"));
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    let mut disabled = draft(account, inbox);
    disabled["enabled"] = json!(false);
    let second = s.run("rules.add", &disabled).unwrap()["id"].as_u64().unwrap();
    let before = serde_json::to_value(&s.mailbox).unwrap();
    assert!(s.run("rules.enable", &json!({"id":second, "value":true})).is_err());
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    // Editing a different enabled predicate into a clash is refused transactionally.
    let mut other = draft(account, inbox);
    other["conditions"][0]["value"] = json!("unrelated@example.org");
    let third = s.run("rules.add", &other).unwrap()["id"].as_u64().unwrap();
    let mut edit = draft(account, inbox);
    edit["id"] = json!(third);
    let before = serde_json::to_value(&s.mailbox).unwrap();
    assert!(s.run("rules.edit", &edit).is_err());
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    let message =
        s.mailbox.add_message(Message { folder: inbox, from: Address::new("Company", "billing@example.org"), ..Default::default() }).unwrap();
    assert!(s.run("rules.fromMessage", &json!({"id":message, "folder":inbox, "server":false})).is_err());
    assert!(s.mailbox.rules.iter().any(|rule| rule.id == first));
    assert_eq!(s.mailbox.message(message).unwrap().folder, inbox);
}

#[test]
fn conflicts_report_grouped_sources_across_destinations_and_preserve_read_only_metadata() {
    let (mut s, account, inbox, archive) = fixture();
    let first = s.run("rules.add", &draft(account, archive)).unwrap()["id"].as_u64().unwrap();
    s.mailbox.rules[0].conditions = vec![Condition::FromAny(vec!["billing@example.org".into(), "news@example.org".into()])];
    let before = serde_json::to_value(&s.mailbox).unwrap();
    let params = json!({"rule":draft(account, inbox)});
    assert_eq!(classify(&s, "rules.conflicts", &params), Access::Safe);
    let report = s.run("rules.conflicts", &params).unwrap();
    assert_eq!(report["confirmed"], 1);
    assert_eq!(report["conflicts"][0]["id"], first);
    assert_eq!(report["conflicts"][0]["destinations"][0]["id"], archive);
    assert_eq!(report["conflicts"][0]["destinations"][0]["account"], account);
    assert_eq!(report["mutated"], false);
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    assert!(s.run("rules.list", &json!({"destination":inbox})).unwrap()["rules"].as_array().unwrap().is_empty());
}

#[test]
fn conflicts_allow_same_destination_copies_and_separate_accounts() {
    let (mut s, account, inbox, archive) = fixture();
    s.run("rules.add", &draft(account, archive)).unwrap();
    let mut same = draft(account, archive);
    same["actions"] = json!([{"type":"moveTo","value":archive},{"type":"markRead"}]);
    s.run("rules.add", &same).unwrap();
    let mut copies = draft(account, inbox);
    copies["actions"] = json!([{"type":"copyTo","value":inbox},{"type":"copyTo","value":archive}]);
    s.run("rules.add", &copies).unwrap();
    let other = s.run("account.add", &json!({"email":"other@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.run("rules.add", &draft(other, inbox)).unwrap();
}

#[test]
fn conflicts_find_a_shared_sender_at_the_end_of_two_full_groups() {
    let (mut s, account, inbox, archive) = fixture();
    let mut a = draft(account, archive);
    let mut b = draft(account, inbox);
    let mut left: Vec<String> = (0..199).map(|i| format!("left-{i}@example.org")).collect();
    let mut right: Vec<String> = (0..199).map(|i| format!("right-{i}@example.org")).collect();
    left.push("shared@example.org".into());
    right.push("SHARED@example.org".into());
    a["conditions"] = json!([{"type":"fromAny","value":left}]);
    b["conditions"] = json!([{"type":"fromAny","value":right}]);
    s.run("rules.add", &a).unwrap();
    assert_eq!(s.run("rules.conflicts", &json!({"rule":b})).unwrap()["confirmed"], 1);
    assert!(s.run("rules.add", &b).is_err());
}

#[test]
fn conflicts_with_complex_regex_stop_execution_before_any_action_and_survive_reopen() {
    let (mut s, account, inbox, archive) = fixture();
    let mut a = draft(account, archive);
    a["conditions"] = json!([{"type":"text","value":{"field":"subject","pattern":"^Invoice", "regex":true}}]);
    a["actions"] = json!([{"type":"markRead"},{"type":"moveTo","value":archive}]);
    let first = s.run("rules.add", &a).unwrap()["id"].as_u64().unwrap();
    let mut b = draft(account, inbox);
    b["conditions"] = json!([{"type":"text","value":{"field":"subject","pattern":"Invoice$", "regex":true}}]);
    s.run("rules.add", &b).unwrap();
    let report = s.run("rules.conflicts", &json!({"id":first})).unwrap();
    assert_eq!(report["confirmed"], 0);
    assert_eq!(report["potential"], 1);
    let message = s.mailbox.add_message(Message { folder: inbox, subject: "Invoice".into(), ..Default::default() }).unwrap();
    s.mailbox = serde_json::from_value(serde_json::to_value(&s.mailbox).unwrap()).unwrap();
    let before = serde_json::to_value(&s.mailbox).unwrap();
    let preview = s.run("rules.preview", &json!({"folder":inbox})).unwrap();
    assert_eq!(preview["failed"], 1);
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    let execution = s.mailbox.apply_rules(message);
    assert!(execution.applied.is_empty());
    assert_eq!(execution.failures.len(), 1);
    assert_eq!(execution.matched, 2);
    assert!(execution.conflicts);
    assert!(execution.failures.iter().all(|failure| failure.completed_actions == 0 && failure.error.contains("conflicting filing")));
    let mut after = s.mailbox.clone();
    after.message_mut(message).unwrap().rule_processing = None;
    assert_eq!(serde_json::to_value(&after).unwrap(), before, "only durable conflict status may change");
    assert_eq!(s.run("rules.pending", &json!({})).unwrap()["messages"][0]["state"]["status"], "conflictingDestinations");
    assert_eq!(s.run("rules.run", &json!({"folder":inbox,"id":first})).unwrap()["failed"], 1);
    assert!(!s.mailbox.message(message).unwrap().read);
}

#[test]
fn conflicts_respect_exceptions_and_draft_preview_keeps_other_destinations_in_scope() {
    let (mut s, account, inbox, archive) = fixture();
    let mut a = draft(account, archive);
    a["exceptions"] = json!([{"type":"subjectContains","value":"Receipt"}]);
    let first = s.run("rules.add", &a).unwrap()["id"].as_u64().unwrap();
    let mut b = draft(account, inbox);
    b["conditions"] = json!([{"type":"fromContains","value":"billing@example.org"},{"type":"subjectContains","value":"Receipt"}]);
    let second = s.run("rules.add", &b).unwrap()["id"].as_u64().unwrap();
    let message = s
        .mailbox
        .add_message(Message { folder: inbox, from: Address::new("", "billing@example.org"), subject: "Receipt".into(), ..Default::default() })
        .unwrap();
    assert!(s.mailbox.apply_rules(message).failures.is_empty());
    assert_eq!(s.mailbox.message(message).unwrap().folder, inbox);
    assert_eq!(s.run("rules.conflicts", &json!({"id":first})).unwrap()["confirmed"], 0);
    let preview = s.run("rules.preview", &json!({"rule":draft(account, inbox), "editing":second, "folder":inbox})).unwrap();
    assert_eq!(preview["failed"], 0, "the original Receipt exception excludes this actual message");
    s.mailbox.message_mut(message).unwrap().subject = "Invoice".into();
    assert_eq!(s.run("rules.preview", &json!({"rule":draft(account,inbox),"editing":second,"folder":inbox})).unwrap()["failed"], 1);
}

#[test]
fn conflicts_include_a_server_rule_still_enabled_while_local_disable_is_pending() {
    let (mut s, account, inbox, archive) = fixture();
    let first = s.run("rules.add", &draft(account, archive)).unwrap()["id"].as_u64().unwrap();
    let server = s.mailbox.rules.iter_mut().find(|rule| rule.id == first).unwrap();
    server.enabled = false;
    server.keep = mailcraft_model::Keep::Server;
    server.server = Some(mailcraft_model::ServerRule { id: "remote-rule".into(), synced_enabled: true, ..Default::default() });
    assert_eq!(s.run("rules.conflicts", &json!({"rule":draft(account,inbox)})).unwrap()["confirmed"], 1);
    assert!(s.run("rules.add", &draft(account, inbox)).is_err());
}

#[test]
fn conflicts_do_not_hide_a_missing_destination_repair_behind_a_false_competing_route() {
    let (mut s, account, inbox, archive) = fixture();
    let first = s.run("rules.add", &draft(account, archive)).unwrap()["id"].as_u64().unwrap();
    let mut invalid = s.mailbox.rules[0].clone();
    invalid.id = 999;
    invalid.stop = false;
    invalid.actions = vec![mailcraft_model::Action::MoveTo(999_999)];
    s.mailbox.rules[0].stop = false;
    s.mailbox.rules.insert(0, invalid);
    let message = s.mailbox.add_message(Message { folder: inbox, from: Address::new("", "billing@example.org"), ..Default::default() }).unwrap();
    let result = s.mailbox.apply_rules(message);
    assert!(!result.conflicts);
    assert_eq!(result.applied.len(), 1);
    assert_eq!(s.mailbox.message(message).unwrap().folder, archive);
    assert!(!s.mailbox.rules.iter().find(|rule| rule.id == 999).unwrap().repair.is_empty());
    assert!(s.mailbox.rules.iter().any(|rule| rule.id == first));
}

#[test]
fn conflicts_treat_delete_and_deleted_subtree_moves_as_one_competing_destination() {
    let (mut s, account, _, archive) = fixture();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.run("rules.add", &draft(account, archive)).unwrap();
    let mut deleting = draft(account, archive);
    deleting["actions"] = json!([{"type":"delete"}]);
    assert!(s.run("rules.add", &deleting).unwrap_err().to_string().contains("filing conflict"));
    let mut into_trash = draft(account, deleted);
    assert!(s.run("rules.add", &into_trash).unwrap_err().to_string().contains("filing conflict"));
    let under = s.mailbox.add_folder(account, Some(deleted), "Old").unwrap();
    let before = serde_json::to_value(&s.mailbox).unwrap();
    assert!(s.run("rules.add", &draft(account, under)).unwrap_err().to_string().contains("filing conflict"));
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    // Delete and a move into Deleted Items are the same destination, not a conflict.
    let mut fresh = fixture();
    let deleted = fresh.0.mailbox.folder_of_kind(fresh.1, FolderKind::Deleted).unwrap();
    deleting = draft(fresh.1, fresh.3);
    deleting["actions"] = json!([{"type":"delete"}]);
    fresh.0.run("rules.add", &deleting).unwrap();
    into_trash = draft(fresh.1, deleted);
    fresh.0.run("rules.add", &into_trash).unwrap();
    let report = fresh.0.run("rules.conflicts", &json!({"rule":draft(fresh.1, fresh.3)})).unwrap();
    assert!(report["confirmed"].as_u64().unwrap() >= 1);
}
