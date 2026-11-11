// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Automatic sender consolidation must preserve filing scope and processing order.

use crate::Session;
use mailcraft_model::{Address, Condition, FolderKind, Message};
use serde_json::{Value, json};

fn fixture() -> (Session, u64, u64, u64) {
    let mut session = Session::empty();
    let account = session.run("account.add", &json!({"email": "source@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let target = session.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    (session, account, inbox, target)
}

fn draft(account: u64, target: u64, sender: &str) -> Value {
    json!({"name": format!("File {sender}"), "account": account, "server": false,
           "conditions": [{"type": "fromContains", "value": sender}],
           "actions": [{"type": "moveTo", "value": target}], "stop": true})
}

fn exact_draft(account: u64, target: u64, sender: &str) -> Value {
    let mut value = draft(account, target, sender);
    value["conditions"] = json!([{"type":"fromAddresses","value":[sender]}]);
    value
}

fn message(session: &mut Session, inbox: u64, sender: &str) -> u64 {
    session.mailbox.add_message(Message { folder: inbox, from: Address::new("Company", sender), ..Default::default() }).unwrap()
}

#[test]
fn automatic_rules_merge_keeps_one_editable_rule_and_original_definitions() {
    let (mut s, account, inbox, target) = fixture();
    let first = s.run("rules.add", &draft(account, target, "billing@example.org")).unwrap()["id"].as_u64().unwrap();
    let second = s.run("rules.add", &draft(account, target, "news@example.org")).unwrap();
    assert_eq!(second["id"], first);
    assert_eq!(s.mailbox.rules.len(), 1);
    let rule = &s.mailbox.rules[0];
    assert_eq!(rule.merged_from.len(), 2);
    assert_eq!(rule.merged_from[0].name, "File billing@example.org");
    assert_eq!(rule.conditions, vec![Condition::FromAny(vec!["billing@example.org".into(), "news@example.org".into()])]);
    let a = message(&mut s, inbox, "BILLING@example.org");
    let b = message(&mut s, inbox, "news@example.org");
    let other = message(&mut s, inbox, "unrelated@example.org");
    let preview = s.run("rules.preview", &json!({"folder": inbox, "id": first})).unwrap();
    assert_eq!(preview["matched"], 2);
    let run = s.run("rules.run", &json!({"folder": inbox, "id": first})).unwrap();
    assert_eq!(run["applied"], 2);
    assert_eq!(s.mailbox.message(a).unwrap().folder, target);
    assert_eq!(s.mailbox.message(b).unwrap().folder, target);
    assert_eq!(s.mailbox.message(other).unwrap().folder, inbox);
    let stored = serde_json::to_vec(&s.mailbox).unwrap();
    let restored: mailcraft_model::Mailbox = serde_json::from_slice(&stored).unwrap();
    assert_eq!(restored.rules, s.mailbox.rules);
    assert_eq!(s.run("rules.list", &json!({"destination": target})).unwrap()["rules"].as_array().unwrap().len(), 1);
}

#[test]
fn automatic_rules_merge_preserves_exact_address_matching_and_provenance() {
    let (mut s, account, inbox, target) = fixture();
    let mut first = draft(account, target, "billing@example.org");
    first["conditions"] = json!([{"type":"fromAddresses","value":["billing@example.org"]}]);
    let id = s.run("rules.add", &first).unwrap()["id"].as_u64().unwrap();
    let mut second = first.clone();
    second["name"] = json!("Other company address");
    second["conditions"][0]["value"] = json!(["news@example.org"]);
    assert_eq!(s.run("rules.add", &second).unwrap()["id"], id);
    assert_eq!(s.mailbox.rules.len(), 1);
    assert!(s.mailbox.rules[0].merged_from.iter().all(|original| original.sender_addresses));
    let good = message(&mut s, inbox, "news@example.org");
    let bad = message(&mut s, inbox, "news@example.org.attacker.invalid");
    s.mailbox.message_mut(bad).unwrap().from.name = "news@example.org".into();
    assert_eq!(s.run("rules.preview", &json!({"folder":inbox,"id":id})).unwrap()["matched"], 1);
    assert_eq!(s.run("rules.run", &json!({"folder":inbox,"id":id})).unwrap()["applied"], 1);
    assert_eq!(s.mailbox.message(good).unwrap().folder, target);
    assert_eq!(s.mailbox.message(bad).unwrap().folder, inbox);
    s.mailbox = serde_json::from_slice(&serde_json::to_vec(&s.mailbox).unwrap()).unwrap();
    assert!(s.mailbox.rules[0].merged_from.iter().all(|original| original.sender_addresses));
    s.run("rules.add", &draft(account, target, "another@example.org")).unwrap();
    assert_eq!(s.mailbox.rules.len(), 2, "substring and exact identity definitions stay distinct");
}

#[test]
fn exact_sender_rules_merge_across_disjoint_sources_without_reordering_or_running_older_mail() {
    let (mut s, account, inbox, target) = fixture();
    let first = s.run("rules.add", &exact_draft(account, target, "billing@example.org")).unwrap()["id"].as_u64().unwrap();
    let mut other = exact_draft(account, target, "other@example.org");
    other["actions"] = json!([{"type":"flag"}]);
    let between = s.run("rules.add", &other).unwrap()["id"].as_u64().unwrap();
    let older = message(&mut s, inbox, "billing@example.org");
    let added = message(&mut s, inbox, "news@example.org");
    let unaffected = message(&mut s, inbox, "other@example.org");
    let mut next = exact_draft(account, target, "news@example.org");
    next["runIn"] = json!(inbox);
    let result = s.run("rules.add", &next).unwrap();
    assert_eq!(result["id"], first);
    assert_eq!(result["execution"]["applied"], 1);
    assert_eq!(s.mailbox.message(older).unwrap().folder, inbox);
    assert_eq!(s.mailbox.message(added).unwrap().folder, target);
    assert_eq!(s.mailbox.message(unaffected).unwrap().flag, mailcraft_model::FlagStatus::None);
    assert_eq!(s.mailbox.rules.iter().map(|rule| rule.id).collect::<Vec<_>>(), [first, between]);
    assert_eq!(s.mailbox.rules[0].merged_from.len(), 2);
    assert!(s.mailbox.rules[0].merged_from.iter().all(|source| source.sender_addresses));
    s.run("rules.run", &json!({"folder":inbox})).unwrap();
    assert_eq!(s.mailbox.message(older).unwrap().folder, target);
    assert_eq!(s.mailbox.message(unaffected).unwrap().folder, inbox);
    assert_eq!(s.mailbox.message(unaffected).unwrap().flag, mailcraft_model::FlagStatus::Flagged);
}

#[test]
fn exact_sender_rules_do_not_merge_across_an_action_that_affects_the_later_sender() {
    for condition in [json!({"type":"fromAddresses","value":["NEWS@example.org"]}), json!({"type":"fromContains","value":"news@example.org"})] {
        let (mut s, account, inbox, target) = fixture();
        s.run("rules.add", &exact_draft(account, target, "billing@example.org")).unwrap();
        let mut other = exact_draft(account, target, "NEWS@example.org");
        other["conditions"] = json!([condition]);
        other["actions"] = json!([{"type":"flag"}]);
        other["stop"] = json!(false);
        s.run("rules.add", &other).unwrap();
        s.run("rules.add", &exact_draft(account, target, "news@example.org")).unwrap();
        assert_eq!(s.mailbox.rules.len(), 3);
        let id = message(&mut s, inbox, "news@example.org");
        s.run("rules.run", &json!({"folder":inbox})).unwrap();
        assert_eq!(s.mailbox.message(id).unwrap().folder, target);
        assert_eq!(s.mailbox.message(id).unwrap().flag, mailcraft_model::FlagStatus::Flagged);
    }
}

#[test]
fn sender_rules_can_merge_past_an_independent_account_without_changing_its_actions() {
    for make in [exact_draft, draft] {
        let (mut s, account, inbox, target) = fixture();
        let other_account = s.run("account.add", &json!({"email":"other@example.net"})).unwrap()["id"].as_u64().unwrap();
        let other_inbox = s.mailbox.folder_of_kind(other_account, FolderKind::Inbox).unwrap();
        let first = s.run("rules.add", &make(account, target, "billing@example.org")).unwrap()["id"].as_u64().unwrap();
        let mut other = draft(other_account, target, "news@example.org");
        other["actions"] = json!([{"type":"flag"}]);
        let between = s.run("rules.add", &other).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.run("rules.add", &make(account, target, "news@example.org")).unwrap()["id"], first);
        assert_eq!(s.mailbox.rules.iter().map(|rule| rule.id).collect::<Vec<_>>(), [first, between]);
        let a = message(&mut s, inbox, "news@example.org");
        let b = message(&mut s, other_inbox, "news@example.org");
        s.run("rules.run", &json!({"folder":inbox})).unwrap();
        s.run("rules.run", &json!({"folder":other_inbox})).unwrap();
        assert_eq!(s.mailbox.message(a).unwrap().folder, target);
        assert_eq!(s.mailbox.message(b).unwrap().folder, other_inbox);
        assert_eq!(s.mailbox.message(b).unwrap().flag, mailcraft_model::FlagStatus::Flagged);
    }
}

#[test]
fn automatic_rules_merge_never_crosses_an_intervening_rule_or_changes_account_scope() {
    let (mut s, account, _, target) = fixture();
    s.run("rules.add", &draft(account, target, "first@example.org")).unwrap();
    let mut intervening = draft(account, target, "middle@example.org");
    intervening["actions"] = json!([{"type": "flag"}]);
    s.run("rules.add", &intervening).unwrap();
    s.run("rules.add", &draft(account, target, "last@example.org")).unwrap();
    assert_eq!(s.mailbox.rules.len(), 3);
    let mut other_scope = draft(account, target, "fourth@example.org");
    other_scope.as_object_mut().unwrap().remove("account");
    s.run("rules.add", &other_scope).unwrap();
    assert_eq!(s.mailbox.rules.len(), 4);
    let mut different_stop = draft(account, target, "fifth@example.org");
    different_stop.as_object_mut().unwrap().remove("account");
    different_stop["stop"] = json!(false);
    s.run("rules.add", &different_stop).unwrap();
    assert_eq!(s.mailbox.rules.len(), 5);
}

#[test]
fn automatic_rules_merge_run_after_create_does_not_file_older_sources() {
    let (mut s, account, inbox, target) = fixture();
    s.run("rules.add", &draft(account, target, "old@example.org")).unwrap();
    let older = message(&mut s, inbox, "old@example.org");
    let added = message(&mut s, inbox, "new@example.org");
    let mut params = draft(account, target, "new@example.org");
    params["runIn"] = json!(inbox);
    let result = s.run("rules.add", &params).unwrap();
    assert_eq!(result["execution"]["applied"], 1);
    assert_eq!(s.mailbox.rules.len(), 1);
    assert_eq!(s.mailbox.message(older).unwrap().folder, inbox);
    assert_eq!(s.mailbox.message(added).unwrap().folder, target);
}

#[test]
fn automatic_rules_merge_from_message_only_runs_the_new_sender() {
    let (mut s, account, inbox, target) = fixture();
    s.run("rules.add", &draft(account, target, "old@example.org")).unwrap();
    let older = message(&mut s, inbox, "old@example.org");
    let new = message(&mut s, inbox, "new@example.org");
    // fromMessage's default stop is false; retain that behavior for both definitions.
    s.mailbox.rules[0].stop = false;
    s.run("rules.fromMessage", &json!({"id": new, "folder": target, "server": false})).unwrap();
    assert_eq!(s.mailbox.rules.len(), 1);
    assert_eq!(s.mailbox.message(older).unwrap().folder, inbox);
    assert_eq!(s.mailbox.message(new).unwrap().folder, target);
}

#[test]
fn automatic_rules_merge_does_not_combine_different_destinations_or_extra_actions() {
    let (mut s, account, inbox, target) = fixture();
    s.run("rules.add", &draft(account, target, "first@example.org")).unwrap();
    s.run("rules.add", &draft(account, inbox, "second@example.org")).unwrap();
    let mut extra = draft(account, inbox, "third@example.org");
    extra["actions"] = json!([{"type":"moveTo", "value":inbox}, {"type":"markRead"}]);
    s.run("rules.add", &extra).unwrap();
    assert_eq!(s.mailbox.rules.len(), 3);
}

#[test]
fn editing_the_sender_set_drops_stale_merge_provenance() {
    let (mut s, account, _, target) = fixture();
    let first = s.run("rules.add", &draft(account, target, "billing@example.org")).unwrap()["id"].as_u64().unwrap();
    s.run("rules.add", &draft(account, target, "news@example.org")).unwrap();
    assert_eq!(s.mailbox.rules[0].merged_from.len(), 2);
    // Renaming alone keeps the original definitions.
    let mut renamed = draft(account, target, "billing@example.org");
    renamed["id"] = json!(first);
    renamed["name"] = json!("Company mail");
    renamed["conditions"] = json!([{"type":"fromAny","value":["billing@example.org","news@example.org"]}]);
    s.run("rules.edit", &renamed).unwrap();
    assert_eq!(s.mailbox.rules[0].merged_from.len(), 2);
    let mut narrowed = draft(account, target, "billing@example.org");
    narrowed["id"] = json!(first);
    s.run("rules.edit", &narrowed).unwrap();
    assert!(s.mailbox.rules[0].merged_from.is_empty());
}
