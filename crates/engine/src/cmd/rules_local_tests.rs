// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local rule behavior, non-mutating previews and exact destination scoping.

use crate::Session;
use mailcraft_model::{Address, FolderKind, Message};
use serde_json::{Value, json};

fn fixture() -> (Session, u64, u64, u64, u64) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "source@example.org"})).unwrap()["id"].as_u64().unwrap();
    let other = s.run("account.add", &json!({"email": "target@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let target = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let message = s
        .mailbox
        .add_message(Message {
            folder: inbox,
            from: Address::new("Billing", "billing@example.org"),
            subject: "Invoice 123".into(),
            ..Default::default()
        })
        .unwrap();
    (s, account, inbox, target, message)
}

fn draft(account: u64, target: u64) -> Value {
    json!({"name": "Invoices", "account": account, "server": false,
        "conditions": [{"type": "text", "value": {"field": "subject", "pattern": "^Invoice [0-9]+$", "regex": true, "caseSensitive": true}}],
        "actions": [{"type": "moveTo", "value": target}], "stop": true})
}

#[test]
fn regex_preview_and_execution_use_the_same_match_without_mutating_the_source() {
    let (mut s, account, inbox, target, message) = fixture();
    let id = s.run("rules.add", &draft(account, target)).unwrap()["id"].as_u64().unwrap();
    let before = serde_json::to_value(&s.mailbox).unwrap();
    let dirty = s.dirty;
    let preview = s.run("rules.preview", &json!({"folder": inbox, "id": id})).unwrap();
    assert_eq!(preview["matched"], 1);
    assert_eq!(preview["messages"][0]["id"], message);
    assert_eq!(preview["messages"][0]["destinationFolder"], target);
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    assert_eq!(s.dirty, dirty);
    assert_eq!(s.run("rules.run", &json!({"folder": inbox, "id": id})).unwrap()["matched"], preview["matched"]);
    assert_eq!(s.mailbox.message(message).unwrap().folder, target);
}

#[test]
fn unavailable_bodies_never_act_as_complete_text_in_preview_or_manual_execution() {
    for unavailable in ["unfetched", "incomplete", "notDownloaded", "storedFailed"] {
        let (mut s, account, inbox, target, message) = fixture();
        let mut params = draft(account, target);
        params["exceptions"] = json!([{"type":"bodyContains","value":"Protected"}]);
        params["actions"] = json!([{"type":"markRead"},{"type":"moveTo","value":target}]);
        let id = s.run("rules.add", &params).unwrap()["id"].as_u64().unwrap();
        let m = s.mailbox.message_mut(message).unwrap();
        m.unfetched = unavailable == "unfetched";
        m.incomplete = unavailable == "incomplete";
        m.not_downloaded = u64::from(unavailable == "notDownloaded");
        if unavailable == "storedFailed" {
            m.kept = Some(Box::new(mailcraft_model::KeptBody { failed: true, ..Default::default() }));
        }
        m.body.text = "A harmless preview".into();
        let before = serde_json::to_value(&s.mailbox).unwrap();
        let preview = s.run("rules.preview", &json!({"folder":inbox,"id":id})).unwrap();
        assert_eq!(preview["failed"], 1, "{unavailable}");
        assert_eq!(preview["matched"], 0);
        assert_eq!(preview["failures"][0]["completedActions"], 0);
        assert!(preview["failures"][0]["error"].as_str().unwrap().contains("complete message content"));
        assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before, "preview is read-only");
        let executed = s.run("rules.run", &json!({"folder":inbox,"id":id})).unwrap();
        assert_eq!(executed["failed"], 1);
        assert_eq!(executed["applied"], 0);
        assert_eq!(s.mailbox.message(message).unwrap().folder, inbox);
        assert!(!s.mailbox.message(message).unwrap().read);
        let m = s.mailbox.message_mut(message).unwrap();
        m.unfetched = false;
        m.incomplete = false;
        m.not_downloaded = 0;
        m.kept = None;
        m.body.text = "Full Protected body".into();
        assert_eq!(s.run("rules.preview", &json!({"folder":inbox,"id":id})).unwrap()["matched"], 0);
        assert_eq!(s.run("rules.run", &json!({"folder":inbox,"id":id})).unwrap()["applied"], 0);
        s.mailbox.message_mut(message).unwrap().body.text = "Full ordinary body".into();
        assert_eq!(s.run("rules.preview", &json!({"folder":inbox,"id":id})).unwrap()["matched"], 1);
        assert_eq!(s.run("rules.run", &json!({"folder":inbox,"id":id})).unwrap()["applied"], 1);
        assert_eq!(s.mailbox.message(message).unwrap().folder, target);
    }
}

#[test]
fn selected_header_rule_waits_for_body_needed_to_check_another_filing_claim() {
    let (mut s, account, inbox, target, message) = fixture();
    let id = s.run("rules.add", &draft(account, target)).unwrap()["id"].as_u64().unwrap();
    // A legacy/imported complex overlap cannot always be proved when definitions are edited.
    s.mailbox.rules.push(mailcraft_model::Rule {
        id: 999,
        account,
        enabled: true,
        conditions: vec![mailcraft_model::Condition::BodyContains("Protected".into())],
        actions: vec![mailcraft_model::Action::MoveTo(inbox)],
        ..Default::default()
    });
    s.mailbox.message_mut(message).unwrap().unfetched = true;
    assert_eq!(s.run("rules.preview", &json!({"folder":inbox,"id":id})).unwrap()["failed"], 1);
    assert_eq!(s.run("rules.run", &json!({"folder":inbox,"id":id})).unwrap()["applied"], 0);
    assert_eq!(s.mailbox.message(message).unwrap().folder, inbox);
}

#[test]
fn case_behavior_exceptions_and_invalid_patterns_are_explicit() {
    let (mut s, account, inbox, target, _) = fixture();
    let mut params = draft(account, target);
    params["conditions"][0]["value"]["pattern"] = json!("^invoice [0-9]+$");
    let id = s.run("rules.add", &params).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.run("rules.preview", &json!({"folder": inbox, "id": id})).unwrap()["matched"], 0);
    params["id"] = json!(id);
    params["conditions"][0]["value"]["caseSensitive"] = json!(false);
    s.run("rules.edit", &params).unwrap();
    assert_eq!(s.run("rules.preview", &json!({"folder": inbox, "id": id})).unwrap()["matched"], 1);
    params["exceptions"] = json!([{"type": "fromContains", "value": "billing@example.org"}]);
    s.run("rules.edit", &params).unwrap();
    assert_eq!(s.run("rules.preview", &json!({"folder": inbox, "id": id})).unwrap()["matched"], 0);
    params["conditions"][0]["value"]["pattern"] = json!("(?=unsupported)");
    let before = serde_json::to_value(&s.mailbox).unwrap();
    assert!(s.run("rules.edit", &params).is_err());
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
}

#[test]
fn destination_lists_include_disabled_rules_without_mixing_same_named_inboxes() {
    let (mut s, account, inbox, target, _) = fixture();
    let id = s.run("rules.add", &draft(account, target)).unwrap()["id"].as_u64().unwrap();
    s.run("rules.enable", &json!({"id": id, "value": false})).unwrap();
    let filtered = s.run("rules.list", &json!({"destination": target})).unwrap();
    assert_eq!(filtered["rules"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["rules"][0]["id"], id);
    assert!(s.run("rules.list", &json!({"destination": inbox})).unwrap()["rules"].as_array().unwrap().is_empty());
}

#[test]
fn oversized_conditions_are_rejected_without_silent_truncation() {
    let (mut s, account, _, target, _) = fixture();
    let mut params = draft(account, target);
    params["conditions"] = json!(vec![json!({"type": "subjectContains", "value": "Invoice"}); 21]);
    assert!(s.run("rules.add", &params).is_err());
    params["conditions"] = json!([{"type": "subjectContains", "value": "x".repeat(201)}]);
    assert!(s.run("rules.add", &params).is_err());
    assert!(s.mailbox.rules.is_empty());
}

#[test]
fn missing_destinations_pause_a_local_rule_and_preview_reports_failure_without_changes() {
    let (mut s, account, inbox, target, message) = fixture();
    let id = s.run("rules.add", &draft(account, target)).unwrap()["id"].as_u64().unwrap();
    let rule = s.mailbox.rules.iter_mut().find(|rule| rule.id == id).unwrap();
    rule.actions = vec![mailcraft_model::Action::MoveTo(999_999), mailcraft_model::Action::Delete];
    let original = serde_json::to_value(&s.mailbox).unwrap();
    let preview = s.run("rules.preview", &json!({"folder": inbox, "id": id})).unwrap();
    assert_eq!(preview["failed"], 1);
    assert_eq!(preview["failures"][0]["completedActions"], 0);
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), original);
    let executed = s.run("rules.run", &json!({"folder": inbox, "id": id})).unwrap();
    assert_eq!(executed["failed"], preview["failed"]);
    assert_eq!(s.mailbox.message(message).unwrap().folder, inbox);
    assert!(!s.mailbox.rules.iter().find(|rule| rule.id == id).unwrap().enabled);
    assert!(s.run("rules.enable", &json!({"id": id, "value": true})).is_err());
    let mut repaired = draft(account, target);
    repaired["id"] = json!(id);
    s.run("rules.edit", &repaired).unwrap();
    assert!(!s.mailbox.rules.iter().find(|rule| rule.id == id).unwrap().enabled, "repair never silently activates a rule");
    s.run("rules.enable", &json!({"id": id, "value": true})).unwrap();
}

#[test]
fn invalid_saved_local_definitions_pause_without_acting_or_rewriting_their_conditions() {
    use mailcraft_model::{Condition, RuleField, RuleText};
    for conditions in [
        vec![Condition::SubjectContains(String::new())],
        Vec::new(),
        vec![Condition::Text(RuleText { field: RuleField::Subject, pattern: "[broken".into(), regex: true, case_sensitive: false })],
    ] {
        let (mut s, account, inbox, target, message) = fixture();
        let id = s.run("rules.add", &draft(account, target)).unwrap()["id"].as_u64().unwrap();
        s.mailbox.rules.iter_mut().find(|r| r.id == id).unwrap().conditions = conditions.clone();
        let original = serde_json::to_value(&s.mailbox).unwrap();
        let preview = s.run("rules.preview", &json!({"folder": inbox, "id": id})).unwrap();
        assert_eq!((preview["matched"].as_u64(), preview["failed"].as_u64()), (Some(0), Some(1)));
        assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), original);
        let result = s.run("rules.run", &json!({"folder": inbox, "id": id})).unwrap();
        assert_eq!(result["failed"], 1);
        assert_eq!(s.mailbox.message(message).unwrap().folder, inbox);
        let stored = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
        assert!(!stored.enabled && !stored.repair.is_empty());
        assert_eq!(stored.conditions, conditions);
    }
}

#[test]
fn an_unconditional_server_original_remains_owned_by_the_server() {
    let (mut s, account, inbox, target, _) = fixture();
    let id = s.run("rules.add", &draft(account, target)).unwrap()["id"].as_u64().unwrap();
    let stored = s.mailbox.rules.iter_mut().find(|r| r.id == id).unwrap();
    stored.conditions.clear();
    stored.keep = mailcraft_model::Keep::Server;
    stored.server = Some(mailcraft_model::ServerRule {
        id: "server-original".into(),
        synced_enabled: true,
        unknown: vec!["conditions".into()],
        ..Default::default()
    });
    let original = stored.clone();
    assert_eq!(s.run("rules.run", &json!({"folder": inbox, "id": id})).unwrap()["failed"], 0);
    assert_eq!(*s.mailbox.rules.iter().find(|r| r.id == id).unwrap(), original);
}

#[test]
fn manual_runs_and_previews_classify_received_mail_only() {
    let (mut s, account, inbox, target, message) = fixture();
    let draft_message = s.mailbox.add_message(Message { folder: inbox, draft: true, subject: "Invoice 456".into(), ..Default::default() }).unwrap();
    let id = s.run("rules.add", &draft(account, target)).unwrap()["id"].as_u64().unwrap();
    let preview = s.run("rules.preview", &json!({"folder": inbox, "id": id})).unwrap();
    assert_eq!((preview["examined"].clone(), preview["skipped"].clone(), preview["matched"].clone()), (json!(1), json!(1), json!(1)));
    let executed = s.run("rules.run", &json!({"folder": inbox, "id": id})).unwrap();
    assert_eq!((executed["examined"].clone(), executed["skipped"].clone()), (json!(1), json!(1)));
    assert_eq!(s.mailbox.message(message).unwrap().folder, target);
    assert_eq!(s.mailbox.message(draft_message).unwrap().folder, inbox);
    // Wholly non-receiving folders are refused with a clear error, before any change.
    for kind in [FolderKind::Sent, FolderKind::Drafts, FolderKind::Deleted, FolderKind::Junk] {
        let folder = s.mailbox.folder_of_kind(account, kind).unwrap();
        s.mailbox.add_message(Message { folder, subject: "Invoice 789".into(), ..Default::default() }).unwrap();
        let before = serde_json::to_value(&s.mailbox).unwrap();
        for command in ["rules.run", "rules.preview"] {
            assert!(s.run(command, &json!({"folder": folder, "id": id})).unwrap_err().to_string().contains("received mail only"), "{kind:?}");
        }
        let mut params = draft(account, target);
        params["runIn"] = json!(folder);
        assert!(s.run("rules.add", &params).unwrap_err().to_string().contains("received mail only"));
        assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
    }
}
