// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Consent follows destruction, rather than the location of a reversible edit.

use serde_json::{Value, json};

use crate::Session;
use crate::agent::{Access, Class, Verdict, check, classify};

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    assert_eq!(check(s, id, &params, &[]), Verdict::Run, "{id}");
    s.agent = Some(Vec::new());
    let result = s.run(id, &params);
    s.agent = None;
    result.unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn read_only_lists_and_options_do_not_need_a_settings_grant() {
    let mut s = sample();
    let before = serde_json::to_value(&s.mailbox).unwrap();
    let prefs = serde_json::to_value(&s.prefs).unwrap();
    for id in ["junk.list", "file.options", "account.list", "rules.list", "file.info"] {
        run(&mut s, id, json!({}));
        assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before, "{id}");
        assert_eq!(serde_json::to_value(&s.prefs).unwrap(), prefs, "{id}");
    }
}

#[test]
fn reversible_account_and_signature_edits_and_their_undo_need_no_consent() {
    let mut s = sample();
    let account = s.mailbox.accounts[0].id;
    let before = s.mailbox.accounts.clone();
    run(&mut s, "account.edit", json!({"id": account, "name": "Personal"}));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.mailbox.accounts, before);
    let signatures = s.mailbox.signatures.clone();
    run(&mut s, "signature.set", json!({"name": "Policy fixture", "text": "Synthetic signature"}));
    let changed = s.mailbox.signatures.clone();
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.mailbox.signatures, signatures);
    run(&mut s, "edit.redo", json!({}));
    assert_eq!(s.mailbox.signatures, changed);
}

#[test]
fn ordinary_cross_account_filing_is_reversible_without_a_send_grant() {
    let mut s = sample();
    let source = s.mailbox.accounts[0].id;
    let inbox = s.mailbox.folder_of_kind(source, mailcraft_model::FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft).unwrap().id;
    let original = s.mailbox.message(id).unwrap().clone();
    let account =
        s.run("account.add", &json!({"email": "other@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let folder = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    run(&mut s, "mail.move", json!({"ids": [id], "folder": folder}));
    assert_eq!(s.mailbox.message(id).unwrap().account, account);
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.mailbox.message(id).unwrap().account, original.account);
    assert_eq!(s.mailbox.message(id).unwrap().folder, original.folder);
}

#[test]
fn removing_account_content_is_a_deletion_not_an_account_edit() {
    let s = sample();
    let account = s.mailbox.accounts[0].id;
    assert!(matches!(check(&s, "account.remove", &json!({"id": account}), &[]), Verdict::Ask(Class::Delete, _)));
}

#[test]
fn public_registration_settings_are_reversible_but_forgetting_secrets_is_destructive() {
    let s = sample();
    for id in ["ui.microsoftClientId", "ui.googleClientId"] {
        assert_eq!(classify(&s, id, &json!({"value": "synthetic-public-identifier"})), Access::Safe);
    }
    assert_eq!(classify(&s, "ui.rememberSignIn", &json!({"value": true})), Access::Safe);
    assert_eq!(classify(&s, "ui.rememberSignIn", &json!({"value": false})), Access::Owner(Class::Delete));
    assert_eq!(crate::agent::access("ui.googleClientSecret"), Some(Access::Never));
    assert_eq!(classify(&s, "ui.confirmServers", &json!({})), Access::Owner(Class::Accounts));
}

#[test]
fn certificate_trust_is_authorization_and_undo_names_the_exact_server_and_pin() {
    let mut s = sample();
    let id = s
        .run("account.add", &json!({"email": "tls@example.org", "kind": "exchange", "ews": {"url": "https://mail.example.org/EWS/Exchange.asmx"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let params = json!({"id": id, "ews": {"trusted": "cd".repeat(32)}});
    assert!(matches!(check(&s, "account.edit", &params, &[]), Verdict::Ask(Class::Accounts, _)));
    s.run("account.edit", &params).unwrap();
    assert_eq!(check(&s, "account.edit", &params, &[]), Verdict::Run, "retaining the same pin grants nothing new");
    run(&mut s, "account.forgetEwsCertificate", json!({"id": id}));
    let what = crate::agent::describe(&s, "edit.undo", &json!({}));
    assert!(what.contains("mail.example.org") && what.contains("CD:CD:CD"), "{what}");
    assert!(matches!(check(&s, "edit.undo", &json!({}), &[]), Verdict::Ask(Class::Accounts, _)));
    let shown = crate::agent::bind::Shown::new(&s, "edit.undo", &json!({}), &what).unwrap();
    assert!(shown.check(&s, "edit.undo", &json!({})).is_ok());
    // A later history step must not reuse the earlier trust question.
    s.run("account.edit", &json!({"id": id, "ews": {"trusted": "ef".repeat(32)}})).unwrap();
    s.run("account.forgetEwsCertificate", &json!({"id": id})).unwrap();
    assert!(shown.check(&s, "edit.undo", &json!({})).is_err());
}

#[test]
fn explicit_private_task_placement_needs_no_extra_send_grant() {
    let mut s = sample();
    let account = s.run("account.add", &json!({"email": "tasks@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let result = run(&mut s, "tasks.new", json!({"account": account, "title": "Personal fixture"}));
    let task = result["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.task(task).unwrap().origin.account, account);
    assert_eq!(s.mailbox.task(task).unwrap().origin.id, "", "placement is queued, not a claimed server write");
    run(&mut s, "tasks.sendLocal", json!({"account": null}));
    assert!(matches!(s.mailbox.local_tasks, mailcraft_model::LocalTasks::NotNow));
}

#[test]
fn discarding_unsaved_signature_text_still_needs_consent() {
    let mut s = sample();
    s.run("signature.set", &json!({"name": "Policy fixture", "text": "Saved"})).unwrap();
    s.run("signature.open", &json!({"name": "Policy fixture"})).unwrap();
    s.run("compose.type", &json!({"text": "Unsaved"})).unwrap();
    assert!(s.signature_edit.as_ref().unwrap().dirty);
    assert_eq!(classify(&s, "signature.close", &json!({"discard": true})), Access::Owner(Class::Delete));
    assert_eq!(classify(&s, "signature.close", &json!({})), Access::Safe);
    for id in ["signature.set", "signature.remove"] {
        let params = json!({"name": "Policy fixture", "text": "Replacement"});
        assert_eq!(classify(&s, id, &params), Access::Owner(Class::Delete), "{id}");
        let shown = crate::agent::bind::Shown::new(&s, id, &params, &crate::agent::describe(&s, id, &params)).unwrap();
        assert!(shown.check(&s, id, &params).is_ok());
        s.run("compose.type", &json!({"text": "Later unsaved typing"})).unwrap();
        assert!(shown.check(&s, id, &params).is_err());
        assert_eq!(classify(&s, id, &json!({"name": "Different signature", "text": "Replacement"})), Access::Safe);
    }
}

#[test]
fn saving_and_opening_a_clean_mailbox_need_no_blanket_mailbox_grant() {
    let mut s = sample();
    s.dirty = false;
    assert_eq!(classify(&s, "file.new", &json!({})), Access::Owner(Class::Delete), "an in-memory mailbox has no rollback file");
    s.path = Some(std::path::PathBuf::from("saved-fixture.mailcraft"));
    for id in ["file.new", "file.open", "file.save", "file.saveAs"] {
        assert_eq!(classify(&s, id, &json!({})), Access::Safe, "{id}");
    }
    s.dirty = true;
    assert_eq!(classify(&s, "file.new", &json!({})), Access::Owner(Class::Delete));
}

#[test]
fn destructive_account_consent_does_not_reach_mail_that_arrives_later() {
    let mut s = sample();
    let account = s.mailbox.accounts[0].id;
    let params = json!({"id": account});
    let shown = crate::agent::bind::Shown::new(&s, "account.remove", &params, &crate::agent::describe(&s, "account.remove", &params)).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    s.mailbox.add_message(mailcraft_model::Message { account, folder: inbox, subject: "New arrival".into(), ..Default::default() }).unwrap();
    s.mailbox_changed();
    assert!(shown.check(&s, "account.remove", &params).is_err());
}

#[test]
fn signature_discard_consent_does_not_reach_later_unsaved_typing() {
    let mut s = sample();
    s.run("signature.set", &json!({"name": "Policy fixture", "text": "Saved"})).unwrap();
    s.run("signature.open", &json!({"name": "Policy fixture"})).unwrap();
    s.run("compose.type", &json!({"text": "First unsaved edit"})).unwrap();
    let params = json!({"discard": true});
    let what = crate::agent::describe(&s, "signature.close", &params);
    assert!(what.contains("Policy fixture") && what.contains("discard unsaved"));
    let shown = crate::agent::bind::Shown::new(&s, "signature.close", &params, &what).unwrap();
    assert!(shown.check(&s, "signature.close", &params).is_ok());
    s.run("compose.type", &json!({"text": "Later typing"})).unwrap();
    assert!(shown.check(&s, "signature.close", &params).is_err());
}

#[test]
fn rules_filing_into_a_shared_mailbox_are_classified_like_moving_mail_there() {
    let mut s = sample();
    let owner = s.mailbox.accounts[0].id;
    let shared = s.mailbox.add_account(mailcraft_model::Account {
        email: "team@example.org".into(),
        kind: mailcraft_model::AccountKind::Imap,
        shared_of: Some(owner),
        ..Default::default()
    });
    let team = s.mailbox.add_folder(shared, None, "Team").unwrap();
    let own = s.run("account.add", &json!({"email": "other@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let own_inbox = s.mailbox.folder_of_kind(own, mailcraft_model::FolderKind::Inbox).unwrap();
    let rule = |action: &str, folder: u64| {
        json!({"name": "Team", "account": owner, "server": false, "conditions": [{"type": "subjectContains", "value": "team fixture"}],
            "actions": [{"type": action, "value": folder}]})
    };
    for action in ["moveTo", "copyTo"] {
        assert_eq!(classify(&s, "rules.add", &rule(action, team)), Access::Owner(Class::Send), "{action}");
        // Filing among the owner's own accounts stays reversible and needs no grant.
        assert_eq!(classify(&s, "rules.add", &rule(action, own_inbox)), Access::Safe, "{action}");
    }
    // Deletion keeps its own, stronger classification.
    let mut deleting = rule("moveTo", team);
    deleting["actions"] = json!([{"type": "moveTo", "value": team}, {"type": "delete"}]);
    assert_eq!(classify(&s, "rules.add", &deleting), Access::Owner(Class::Delete));
    let mut disabled = rule("moveTo", team);
    disabled["enabled"] = json!(false);
    let made = s.run("rules.add", &disabled).unwrap()["id"].as_u64().unwrap();
    assert_eq!(classify(&s, "rules.enable", &json!({"id": made, "value": true})), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "rules.enable", &json!({"id": made, "value": false})), Access::Safe);
}
