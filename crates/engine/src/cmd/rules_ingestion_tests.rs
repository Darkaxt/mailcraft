// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Classification follows first detection and content completion, not message date or a sweep.

use super::graph_down::{self, Down, ServerIndex};
use crate::Session;
use mailcraft_model::sync::{EwsMessagePage, FolderDelta, GraphMessage, MessagePage, ServerMessage, SyncResult};
use mailcraft_model::{AccountKind, Action, Address, Body, Condition, FolderKind, Id, Importance, Message, Rule, ServerFolder};
use serde_json::json;

fn fixture() -> (Session, Id, Id, Id) {
    let mut s = Session::new(mailcraft_model::Mailbox::with_local_account("source@example.org", "Source"));
    s.clock.now = Some(1_794_000_000);
    let account = s.mailbox.accounts[0].id;
    s.mailbox.accounts[0].kind = AccountKind::Microsoft;
    let receiving = s.mailbox.add_folder(account, None, "Already filed elsewhere").unwrap();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    for (folder, server) in [(receiving, "FReceive"), (archive, "FArchive")] {
        s.mailbox.folder_mut(folder).unwrap().server = Some(ServerFolder { id: server.into(), ..Default::default() });
    }
    (s, account, receiving, archive)
}

fn page() -> MessagePage {
    MessagePage {
        folder: "FReceive".into(),
        afresh: true,
        messages: vec![GraphMessage {
            server: ServerMessage { id: "M1".into(), folder: "FReceive".into(), size: 1000, ..Default::default() },
            message_id: "old-message@example.org".into(),
            subject: "Invoice".into(),
            received: 1,
            from: Address::new("Company", "billing@example.org"),
            preview: "A short preview".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn add_rule(s: &mut Session, account: Id, destination: Id, body: bool, copy: bool) {
    let condition =
        if body { json!({"type":"bodyContains","value":"Only in the full body"}) } else { json!({"type":"subjectContains","value":"Invoice"}) };
    s.run(
        "rules.add",
        &json!({"name":"Company filing","account":account,"server":false,
        "conditions":[condition],"actions":[{"type":if copy {"copyTo"} else {"moveTo"},"value":destination}]}),
    )
    .unwrap();
}

#[test]
fn rules_ingestion_classifies_initial_historical_custom_folder_pages_in_all_three_adapters_once() {
    for adapter in ["graph", "exchange", "jmap"] {
        let (mut s, account, receiving, archive) = fixture();
        add_rule(&mut s, account, archive, false, false);
        let mut index = ServerIndex::default();
        for replay in [false, true] {
            let mut down = Down::default();
            match adapter {
                "graph" => graph_down::take_page(&mut s.mailbox, &mut index, account, &page(), &mut down),
                "exchange" => {
                    super::ews_down::take_page(
                        &mut s.mailbox,
                        &mut index,
                        account,
                        &EwsMessagePage { folder: "FReceive".into(), afresh: true, messages: page().messages, ..Default::default() },
                        &mut down,
                    );
                }
                _ => super::jmap_down::take_window(&mut s.mailbox, &mut index, account, &page(), true, &mut down),
            }
            assert_eq!(s.mailbox.messages.len(), 1, "{adapter}, replay={replay}");
            let message = s.mailbox.messages.values().next().unwrap();
            assert_eq!(message.folder, archive, "{adapter}, replay={replay}");
            assert_ne!(message.folder, receiving);
            assert_eq!(down.new, usize::from(!replay));
        }
    }
}

#[test]
fn rules_ingestion_waits_for_full_body_survives_reopen_and_does_not_repeat_a_copy() {
    let (mut s, account, receiving, archive) = fixture();
    add_rule(&mut s, account, archive, true, true);
    s.run(
        "rules.add",
        &json!({"name":"Read after classification","account":account,"server":false,
        "conditions":[{"type":"subjectContains","value":"Invoice"}],"actions":[{"type":"markRead"}]}),
    )
    .unwrap();
    let mut index = ServerIndex::default();
    graph_down::take_page(&mut s.mailbox, &mut index, account, &page(), &mut Down::default());
    let message = s.mailbox.messages.values().next().unwrap().id;
    assert!(s.mailbox.message(message).unwrap().rules_wait_for_content());
    assert!(!s.mailbox.message(message).unwrap().read, "header-only actions wait for an unresolved body condition too");
    assert_eq!(s.run("rules.pending", &json!({})).unwrap()["waitingForContent"], 1);
    s.mailbox = serde_json::from_value(serde_json::to_value(&s.mailbox).unwrap()).unwrap();
    assert_eq!(graph_down::fetch_queue(&s.mailbox, account, 10, s.now()), ["M1"], "required content is fetched even outside the offline age window");
    assert_eq!(
        super::jmap_down::fetch_queue(&s.mailbox, account, 10, &Default::default()),
        ["M1"],
        "JMAP is not limited to its newest Inbox messages for this work"
    );
    let mime = b"From: billing@example.org\r\nSubject: Invoice\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nOnly in the full body.\r\n";
    index = ServerIndex::default();
    assert_eq!(graph_down::take_fetched_as(&mut s.mailbox, &mut index, account, "M1", mime), Some(message));
    assert!(s.mailbox.message(message).unwrap().read);
    assert!(s.mailbox.message(message).unwrap().rule_processing.is_none());
    assert_eq!(s.mailbox.messages_in(archive).count(), 1);
    assert_eq!(s.mailbox.messages_in(receiving).count(), 1);
    let queue = s.mailbox.queued.clone();
    graph_down::take_fetched_as(&mut s.mailbox, &mut index, account, "M1", mime);
    graph_down::take_page(&mut s.mailbox, &mut index, account, &page(), &mut Down::default());
    assert_eq!(s.mailbox.messages_in(archive).count(), 1);
    assert_eq!(s.mailbox.queued, queue, "a replay cannot queue a second copy");
    assert!(s.mailbox.messages.values().all(|message| message.rule_processing.is_none()));
}

#[test]
fn rules_ingestion_does_not_turn_a_missing_body_exception_into_a_match() {
    let (mut s, account, receiving, archive) = fixture();
    s.run(
        "rules.add",
        &json!({"name":"Invoice except protected","account":account,"server":false,
        "conditions":[{"type":"subjectContains","value":"Invoice"}],
        "exceptions":[{"type":"bodyContains","value":"Protected"}],"actions":[{"type":"moveTo","value":archive}]}),
    )
    .unwrap();
    let mut index = ServerIndex::default();
    graph_down::take_page(&mut s.mailbox, &mut index, account, &page(), &mut Down::default());
    let message = s.mailbox.messages.values().next().unwrap().id;
    assert!(s.mailbox.message(message).unwrap().rules_wait_for_content());
    graph_down::take_fetched_as(&mut s.mailbox, &mut index, account, "M1", b"Subject: Invoice\r\n\r\nProtected message.");
    assert_eq!(s.mailbox.message(message).unwrap().folder, receiving);
    assert!(s.mailbox.message(message).unwrap().rule_processing.is_none());
}

#[test]
fn rules_ingestion_imap_historical_custom_folder_cross_account_move_is_durable_and_deduplicated() {
    let mut s = Session::empty();
    let mut accounts = Vec::new();
    for email in ["source@example.org", "target@example.org"] {
        accounts.push(s.run("account.add", &json!({"email":email,"incoming":{"host":"imap.example.org"},"outgoing":{"host":"smtp.example.org"}})).unwrap()["id"].as_u64().unwrap());
    }
    let (account, target) = (accounts[0], accounts[1]);
    let receiving = s.mailbox.add_folder(account, None, "Historical receiving folder").unwrap();
    s.mailbox.folder_mut(receiving).unwrap().remote = "OldMail".into();
    let destination = s.mailbox.folder_of_kind(target, FolderKind::Inbox).unwrap();
    super::account::apply_sync(
        &mut s,
        SyncResult {
            account: target,
            folders: vec![FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, ..Default::default() }],
            ..Default::default()
        },
    )
    .unwrap();
    add_rule(&mut s, account, destination, false, false);
    let delta = FolderDelta {
        id: Some(receiving),
        remote: "OldMail".into(),
        kind: FolderKind::Custom,
        uid_validity: 8,
        uid_seen: 5,
        new: vec![Message {
            uid: 5,
            received: 1,
            subject: "Invoice".into(),
            message_id: "old@example.org".into(),
            from: Address::new("Company", "billing@example.org"),
            body: Body { text: "Whole body".into(), html: None },
            ..Default::default()
        }],
        ..Default::default()
    };
    super::account::apply_sync(&mut s, SyncResult { account, folders: vec![delta.clone()], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.messages_in(destination).count(), 1);
    assert_eq!(s.mailbox.messages_in(receiving).count(), 0);
    assert_eq!(s.mailbox.queued.len(), 1);
    s.mailbox = serde_json::from_value(serde_json::to_value(&s.mailbox).unwrap()).unwrap();
    super::account::apply_sync(&mut s, SyncResult { account, folders: vec![delta], ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.messages_in(destination).count(), 1);
    assert_eq!(s.mailbox.messages.len(), 1);
    assert_eq!(s.mailbox.queued.len(), 1);
}

#[test]
fn rules_ingestion_does_not_sweep_sent_drafts_trash_or_junk() {
    let (mut s, account, _, archive) = fixture();
    add_rule(&mut s, account, archive, false, false);
    for kind in [FolderKind::Sent, FolderKind::Drafts, FolderKind::Deleted, FolderKind::Junk, FolderKind::Outbox] {
        let folder = s.mailbox.folder_of_kind(account, kind).unwrap();
        let child = s.mailbox.add_folder(account, Some(folder), "Child").unwrap();
        for folder in [folder, child] {
            let id = s.mailbox.add_message(Message { folder, subject: "Invoice".into(), ..Default::default() }).unwrap();
            assert!(s.mailbox.apply_rules(id).applied.is_empty());
            assert_eq!(s.mailbox.message(id).unwrap().folder, folder);
        }
    }
}

#[test]
fn rules_ingestion_prevents_competing_moves_caused_by_earlier_importance_changes() {
    let (mut s, account, receiving, archive) = fixture();
    s.mailbox.rules = vec![
        Rule {
            id: 500,
            name: "Make low".into(),
            enabled: true,
            account,
            conditions: vec![Condition::Importance(Importance::High)],
            actions: vec![Action::SetImportance(Importance::Low), Action::MoveTo(archive)],
            ..Default::default()
        },
        Rule {
            id: 501,
            name: "Already low".into(),
            enabled: true,
            account,
            conditions: vec![Condition::Importance(Importance::Low)],
            actions: vec![Action::MoveTo(receiving)],
            ..Default::default()
        },
    ];
    let id = s.mailbox.add_message(Message { folder: receiving, importance: Importance::High, ..Default::default() }).unwrap();
    let result = s.mailbox.apply_rules(id);
    assert!(result.conflicts && result.applied.is_empty());
    assert_eq!(s.mailbox.message(id).unwrap().folder, receiving);
    assert_eq!(s.mailbox.message(id).unwrap().importance, Importance::High);
    // Ordinary sequential non-conflicting actions keep their original behavior.
    s.mailbox.rules[0].actions = vec![Action::SetImportance(Importance::Low), Action::MarkRead];
    assert_eq!(s.mailbox.apply_rules(id).applied, ["Make low", "Already low"]);
    assert!(s.mailbox.message(id).unwrap().read);
}

#[test]
fn rules_ingestion_waits_for_body_exceptions_after_an_earlier_importance_change() {
    let (mut s, account, receiving, archive) = fixture();
    s.mailbox.rules = vec![
        Rule {
            id: 500,
            name: "Raise importance".into(),
            account,
            enabled: true,
            conditions: vec![Condition::SubjectContains("Invoice".into())],
            actions: vec![Action::SetImportance(Importance::High)],
            ..Default::default()
        },
        Rule {
            id: 501,
            name: "Important except protected".into(),
            account,
            enabled: true,
            conditions: vec![Condition::Importance(Importance::High), Condition::Unless(vec![Condition::BodyContains("Protected".into())])],
            actions: vec![Action::MoveTo(archive)],
            ..Default::default()
        },
    ];
    graph_down::take_page(&mut s.mailbox, &mut ServerIndex::default(), account, &page(), &mut Down::default());
    let id = s.mailbox.messages.values().next().unwrap().id;
    assert!(s.mailbox.message(id).unwrap().rules_wait_for_content());
    assert_eq!(s.mailbox.message(id).unwrap().importance, Importance::Normal);
    assert_eq!(s.mailbox.message(id).unwrap().folder, receiving);
    let mime = b"From: billing@example.org\r\nSubject: Invoice\r\nContent-Type: text/plain\r\n\r\nProtected\r\n";
    graph_down::take_fetched_as(&mut s.mailbox, &mut ServerIndex::default(), account, "M1", mime);
    assert_eq!(s.mailbox.message(id).unwrap().importance, Importance::High);
    assert_eq!(s.mailbox.message(id).unwrap().folder, receiving);
    assert!(!s.mailbox.message(id).unwrap().rules_wait_for_content());
}

#[test]
fn rules_ingestion_pending_totals_cover_all_messages_and_manual_relocation_cancels_waiting() {
    let (mut s, account, receiving, archive) = fixture();
    add_rule(&mut s, account, archive, true, true);
    let mut batch = page();
    let original = batch.messages[0].clone();
    batch.messages = (0..150)
        .map(|i| {
            let mut message = original.clone();
            message.server.id = format!("M{i}");
            message
        })
        .collect();
    graph_down::take_page(&mut s.mailbox, &mut ServerIndex::default(), account, &batch, &mut Down::default());
    let report = s.run("rules.pending", &json!({"limit":1})).unwrap();
    assert_eq!(report["total"], 150);
    assert_eq!(report["waitingForContent"], 150);
    assert_eq!(report["messages"].as_array().unwrap().len(), 1);
    assert_eq!(report["detailsTruncated"], true);
    let id = s.mailbox.messages_in(receiving).next().unwrap().id;
    s.mailbox.move_message(id, archive).unwrap();
    assert!(s.mailbox.message(id).unwrap().rule_processing.is_none());
    assert_eq!(s.run("rules.pending", &json!({"limit":1})).unwrap()["total"], 149);
}
