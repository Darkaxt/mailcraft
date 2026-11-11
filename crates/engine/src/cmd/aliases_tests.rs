// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native alias view, reply routing, exact predicates and saved-mailbox integration.

use std::sync::Arc;

use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::{
    Session,
    agent::{self, Access},
};

fn message(folder: Id, alias: &str, reply_to: bool) -> Message {
    Message {
        folder,
        from: Address::new("Sender through SimpleLogin", "sender_random@simplelogin.co"),
        reply_to: if reply_to { vec![Address::new("", "reply_random@simplelogin.co")] } else { Vec::new() },
        to: vec![Address::new("", alias)],
        headers: vec![
            ("X-SimpleLogin-Type".into(), "Forward".into()),
            ("X-SimpleLogin-Envelope-To".into(), alias.into()),
            ("X-SimpleLogin-Original-From".into(), "Sender <sender@shop.example>".into()),
            ("Received".into(), "from mail-200.simplelogin.co (mail-200.simplelogin.co [192.0.2.10]) by mx.example.org".into()),
        ],
        body: mailcraft_model::Body { text: "A retained offline message".into(), html: None },
        ..Default::default()
    }
}

fn fixture() -> (Session, Id, Id, Id, Id) {
    let mut session = Session::empty();
    let first = session.run("account.add", &json!({"email": "first@example.org"})).unwrap()["id"].as_u64().unwrap();
    let second = session.run("account.add", &json!({"email": "second@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = session.mailbox.folder_of_kind(first, FolderKind::Inbox).unwrap();
    let another = session.mailbox.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let custom = session.mailbox.add_folder(first, Some(inbox), "Company").unwrap();
    let one = session.mailbox.add_message(message(inbox, "shop@aliases.example", true)).unwrap();
    session.mailbox.add_message(message(custom, "shop@aliases.example", false)).unwrap();
    session.mailbox.add_message(message(inbox, "news@aliases.example", false)).unwrap();
    session.mailbox.add_message(message(another, "shop@aliases.example", false)).unwrap();
    session.mailbox_changed();
    (session, first, second, inbox, one)
}

#[test]
fn simple_login_alias_inventory_view_and_reply_are_one_read_only_workflow() {
    let (mut session, account, other, inbox, one) = fixture();
    let before = serde_json::to_value(&session.mailbox).unwrap();
    for command in ["aliases.list", "mail.identity", "view.alias", "ui.toggleAliases"] {
        assert_eq!(agent::classify(&session, command, &json!({})), Access::Safe, "{command}");
    }
    let inventory = session.run("aliases.list", &json!({"account": account, "offset": 0, "limit": 1})).unwrap();
    assert_eq!(inventory["total"], 2);
    assert_eq!(inventory["aliases"].as_array().unwrap().len(), 1);
    assert_eq!(inventory["nextOffset"], 1);
    let next = session.run("aliases.list", &json!({"account": account, "offset": 1, "limit": 1})).unwrap();
    assert!(next["nextOffset"].is_null());
    assert_eq!(next["aliases"][0]["total"], 2);
    let other_inventory = session.run("aliases.list", &json!({"account": other})).unwrap();
    assert_eq!(other_inventory["aliases"][0]["total"], 1);
    let identity = session.run("mail.identity", &json!({"id": one})).unwrap();
    assert_eq!(identity["transportFrom"]["email"], "sender_random@simplelogin.co");
    assert_eq!(identity["replyTo"][0]["email"], "reply_random@simplelogin.co");
    assert_eq!(identity["simpleLogin"]["originalFrom"]["email"], "sender@shop.example");
    assert_eq!(identity["simpleLogin"]["authenticated"], false);
    let view = session.run("view.alias", &json!({"account": account, "alias": "SHOP@aliases.example"})).unwrap();
    assert_eq!(view["found"], 2);
    assert!(session.rows().iter().all(|row| session.mailbox.message(row.id).unwrap().account == account));
    assert_eq!(serde_json::to_value(&session.mailbox).unwrap(), before);
    session.run("mail.reply", &json!({"id": one})).unwrap();
    assert_eq!(session.compose.take().unwrap().to, "reply_random@simplelogin.co");
    let without_reply = session.mailbox.messages.values().find(|message| message.account == account && message.reply_to.is_empty()).unwrap().id;
    session.run("mail.reply", &json!({"id": without_reply})).unwrap();
    assert_eq!(session.compose.take().unwrap().to, "Sender through SimpleLogin <sender_random@simplelogin.co>");
    session.run("folder.select", &json!({"id": inbox})).unwrap();
    assert!(session.view.search.is_empty());
}

#[test]
fn simple_login_coverage_cache_deleted_descendants_and_invalid_parameters_are_explicit() {
    let (mut session, account, _, inbox, _) = fixture();
    let deleted = session.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let descendant = session.mailbox.add_folder(account, Some(deleted), "Old company").unwrap();
    session.mailbox.add_message(message(descendant, "retired@aliases.example", false)).unwrap();
    let mut invalid = message(inbox, "ambiguous@aliases.example", false);
    invalid.headers.push(("X-SimpleLogin-Envelope-To".into(), "other@aliases.example".into()));
    session.mailbox.add_message(invalid).unwrap();
    session.mailbox_changed();
    let first = session.aliases();
    assert!(Arc::ptr_eq(&first, &session.aliases()));
    let inventory = session.run("aliases.list", &json!({"account": account})).unwrap();
    assert_eq!(inventory["total"], 2);
    assert_eq!(inventory["unresolvedAlias"], 1);
    session.mailbox.add_message(message(inbox, "new@aliases.example", false)).unwrap();
    session.mailbox_changed();
    assert!(!Arc::ptr_eq(&first, &session.aliases()));
    let before = serde_json::to_value(&session.mailbox).unwrap();
    let view = session.view.clone();
    for params in [
        json!({"account": account,"alias": "not an address"}),
        json!({"account": 0,"alias": "shop@aliases.example"}),
        json!({"account": account,"alias": "shop@aliases.example", "move": true}),
    ] {
        assert!(session.run("view.alias", &params).is_err());
        assert_eq!(session.view, view);
        assert_eq!(serde_json::to_value(&session.mailbox).unwrap(), before);
    }
    assert!(session.run("aliases.list", &json!({"account":account,"offset":null})).is_err());
    assert!(session.run("aliases.list", &json!({"account":account,"limit":0})).is_err());
}

#[test]
fn simple_login_predicates_and_identity_survive_native_save_reopen_without_network() {
    let (mut session, account, _, inbox, one) = fixture();
    let target = session.mailbox.add_folder(account, None, "Filed").unwrap();
    let made = session
        .run(
            "rules.add",
            &json!({"name":"Alias filing","account":account,"server":false,
        "conditions":[{"type":"text","value":{"field":"receivingAlias","pattern":"^shop@aliases\\.example$","regex":true}},
                      {"type":"text","value":{"field":"originalFrom","pattern":"^sender@shop\\.example$","regex":true}}],
        "actions":[{"type":"moveTo","value":target}]}),
        )
        .unwrap();
    let stored = crate::io::write_mailbox(&session.mailbox).unwrap();
    let mut reopened = Session::new(crate::io::read_mailbox(&stored).unwrap());
    let identity = reopened.run("mail.identity", &json!({"id":one})).unwrap();
    assert_eq!(identity["simpleLogin"]["receivingAlias"], "shop@aliases.example");
    let preview = reopened.run("rules.preview", &json!({"folder":inbox,"id":made["id"]})).unwrap();
    assert_eq!(preview["matched"], 1);
    assert_eq!(preview["messages"][0]["destinationFolder"], target);
    assert_eq!(reopened.mailbox.message(one).unwrap().folder, inbox);
    let run = reopened.run("rules.run", &json!({"folder":inbox,"id":made["id"]})).unwrap();
    assert_eq!(run["matched"], 1);
    assert_eq!(reopened.mailbox.message(one).unwrap().folder, target);
    reopened.run("view.alias", &json!({"account":account,"alias":"shop@aliases.example"})).unwrap();
    assert_eq!(reopened.rows().len(), 2);
}

#[test]
fn simple_login_literal_predicates_cannot_activate_competing_destinations() {
    for (field, pattern) in [("receivingAlias", "shop@aliases.example"), ("originalFrom", "sender@shop.example")] {
        let (mut session, account, _, inbox, _) = fixture();
        let target = session.mailbox.add_folder(account, None, "Company").unwrap();
        let mut draft = json!({"name":"Identity filing","account":account,"server":false,
            "conditions":[{"type":"text","value":{"field":field,"pattern":pattern}}],
            "actions":[{"type":"moveTo","value":target}],"stop":true});
        session.run("rules.add", &draft).unwrap();
        draft["actions"][0]["value"] = json!(inbox);
        let before = serde_json::to_value(&session.mailbox).unwrap();
        let conflicts = session.run("rules.conflicts", &json!({"rule":draft})).unwrap();
        assert_eq!(conflicts["confirmed"], 1, "{field}");
        assert!(session.run("rules.add", &draft).is_err());
        assert_eq!(serde_json::to_value(&session.mailbox).unwrap(), before);
        draft["enabled"] = json!(false);
        let second = session.run("rules.add", &draft).unwrap()["id"].as_u64().unwrap();
        let before = serde_json::to_value(&session.mailbox).unwrap();
        assert!(session.run("rules.enable", &json!({"id":second,"value":true})).is_err());
        assert_eq!(serde_json::to_value(&session.mailbox).unwrap(), before);
    }
}

#[test]
fn spoofed_simple_login_headers_give_no_alias_folder_colour_or_original_sender() {
    let (mut session, account, _, inbox, _) = fixture();
    let mut spoofed = message(inbox, "shop@aliases.example", false);
    spoofed.headers.retain(|(name, _)| name != "Received");
    spoofed.headers.push(("Received".into(), "from mx.spam.example (mx.spam.example [192.0.2.66]) by mx.example.org".into()));
    let spoofed = session.mailbox.add_message(spoofed).unwrap();
    let mut misaddressed = message(inbox, "shop@aliases.example", false);
    misaddressed.to = vec![Address::new("", "first@example.org")];
    let misaddressed = session.mailbox.add_message(misaddressed).unwrap();
    session.mailbox_changed();
    session.run("aliases.rowColor", &json!({"account": account, "alias": "shop@aliases.example", "color": "#D5EEDC"})).unwrap();
    let inventory = session.run("aliases.list", &json!({"account": account})).unwrap();
    assert_eq!(inventory["aliases"][0]["address"], "news@aliases.example");
    assert_eq!(inventory["aliases"][1]["total"], 2);
    assert_eq!(inventory["unresolvedAlias"], 2);
    let view = session.run("view.alias", &json!({"account": account, "alias": "shop@aliases.example"})).unwrap();
    assert_eq!(view["found"], 2);
    for id in [spoofed, misaddressed] {
        let identity = session.run("mail.identity", &json!({"id": id})).unwrap();
        assert_eq!(identity["simpleLogin"]["corroborated"], false);
        assert!(identity["simpleLogin"]["receivingAlias"].is_null());
        assert!(identity["simpleLogin"]["originalFrom"].is_null());
        let message = session.mailbox.message(id).unwrap();
        assert!(message.origin_alias().is_none());
        let origin = session.mailbox.account(message.origin_account()).unwrap();
        assert_eq!(origin.origin_row_color(message.origin_alias().as_deref()), origin.effective_row_color());
    }
}
