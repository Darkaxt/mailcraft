// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real authenticated MCP traffic through the windowless native mail pump.

use std::net::TcpListener;

use mailcraft_engine::Session;
use mailcraft_mcp::{Remote, Server};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::{Value, json};

use super::{context, pump};

fn tool(server: &mut Server, id: usize, name: &str) -> Result<Value, String> {
    tool_args(server, id, name, json!({}))
}

fn tool_args(server: &mut Server, id: usize, name: &str, arguments: Value) -> Result<Value, String> {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}});
    let reply = server.handle_line(&line.to_string()).ok_or("missing MCP reply")?;
    let reply: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    if reply["result"]["isError"] != false {
        return Err(reply.to_string());
    }
    let text = reply["result"]["content"][0]["text"].as_str().ok_or("missing MCP text")?;
    serde_json::from_str(text).map_err(|e| e.to_string())
}

#[test]
fn real_mcp_reads_and_normal_close_need_no_native_window() {
    let mut session = Session::new(mailcraft_engine::sample::mailbox(0, 0));
    // This fixture proves a new filing definition; competing definitions are exercised below.
    session.mailbox.rules.clear();
    let inbox = session.mailbox.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Inbox).unwrap().id;
    // A synthetic server-backed destination proves the filing exemption through
    // the actual MCP/agent dispatcher, without a provider connection or live mail.
    let destination_account = session
        .run(
            "account.add",
            &json!({"email":"filing@example.org",
        "incoming":{"host":"imap.example.org"}, "outgoing":{"host":"smtp.example.org"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    mailcraft_engine::cmd::account::apply_sync(
        &mut session,
        mailcraft_model::sync::SyncResult {
            account: destination_account,
            folders: vec![mailcraft_model::sync::FolderDelta {
                remote: "INBOX".into(),
                kind: mailcraft_model::FolderKind::Inbox,
                uid_validity: 1,
                ..Default::default()
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let archive = session.mailbox.folder_of_kind(destination_account, mailcraft_model::FolderKind::Inbox).unwrap();
    let message = session.mailbox.messages_in(inbox).next().unwrap();
    let (message_id, subject, account) = (message.id, message.subject.clone(), message.account);
    assert_ne!(account, destination_account);
    let (ctx, wake) = context();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let key = "synthetic-headless-key";
    let requests = crate::control_server::serve_on(listener, ctx.clone(), key.into(), std::time::Duration::from_secs(30), true);
    let backend = Remote::connect_with_key(&address, key).unwrap().without_replay();
    let client = std::thread::spawn(move || {
        let mut server = Server::new(Box::new(backend));
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let status = tool(&mut server, 1, "status").unwrap();
            assert_eq!(status["lifecycle"]["mode"], "headless");
            assert_eq!(status["lifecycle"]["nativeWindow"], false);
            assert_eq!(status["lifecycle"]["openingContentPending"], false);
            let mailbox = tool(&mut server, 2, "inspect_mailbox").unwrap();
            assert!(mailbox["messages"].as_u64().unwrap() > 0);
            assert!(!mailbox["accounts"].as_array().unwrap().is_empty());
            let commands = tool_args(&mut server, 3, "list_commands", json!({"query":"rules"})).unwrap();
            for id in [
                "rules.add",
                "rules.edit",
                "rules.list",
                "rules.preview",
                "rules.conflicts",
                "rules.pending",
                "rules.run",
                "rules.enable",
                "rules.remove",
            ] {
                let command = commands.as_array().unwrap().iter().find(|command| command["id"] == id).unwrap();
                assert_eq!(command["agent"], "safe", "ordinary filing discovery: {id}");
                assert!(command.get("needs").is_none());
            }
            let mut definition = json!({"name":"Headless filing", "account": account, "server":false,
                "conditions":[{"type":"subjectContains","value":subject}], "actions":[{"type":"moveTo","value":archive}]});
            let made = tool_args(&mut server, 4, "execute", json!({"command":"rules.add","params":definition})).unwrap();
            let rule = made["id"].as_u64().unwrap();
            let conflicts = tool_args(&mut server, 31, "execute", json!({"command":"rules.conflicts","params":{"id":rule}})).unwrap();
            assert_eq!(conflicts["confirmed"], 0);
            let mut competing = definition.clone();
            competing["actions"] = json!([{"type":"moveTo","value":inbox}]);
            assert!(
                tool_args(&mut server, 32, "execute", json!({"command":"rules.add","params":competing})).unwrap_err().contains("filing conflict")
            );
            let pending = tool_args(&mut server, 33, "execute", json!({"command":"rules.pending","params":{}})).unwrap();
            assert_eq!(pending["total"], 0);
            let listed = tool_args(&mut server, 5, "execute", json!({"command":"rules.list","params":{"destination":archive}})).unwrap();
            assert!(listed["rules"].as_array().unwrap().iter().any(|r| r["id"] == rule));
            assert_eq!(listed["pendingDeletionCount"], 0);
            let folders = tool_args(&mut server, 34, "execute", json!({"command":"folder.list","params":{}})).unwrap();
            let destination = folders["accounts"].as_array().unwrap().iter().find(|entry| entry["id"] == destination_account).unwrap();
            let target = destination["folders"].as_array().unwrap().iter().find(|entry| entry["id"] == archive).unwrap();
            assert_eq!(target["providerIdentity"]["imapPath"], "INBOX");
            let preview = tool_args(&mut server, 6, "execute", json!({"command":"rules.preview","params":{"folder":inbox,"id":rule}})).unwrap();
            assert!(preview["matched"].as_u64().unwrap() > 0);
            assert_eq!(preview["mutated"], false);
            definition["id"] = json!(rule);
            definition["name"] = json!("Headless filing updated");
            tool_args(&mut server, 7, "execute", json!({"command":"rules.edit","params":definition})).unwrap();
            tool_args(&mut server, 8, "execute", json!({"command":"rules.enable","params":{"id":rule,"value":false}})).unwrap();
            tool_args(&mut server, 9, "execute", json!({"command":"rules.enable","params":{"id":rule,"value":true}})).unwrap();
            let filed = tool_args(&mut server, 10, "execute", json!({"command":"rules.run","params":{"id":rule,"folder":inbox}})).unwrap();
            assert!(filed["applied"].as_u64().unwrap() > 0);
            assert_eq!(filed["failed"], 0);
            let filed_message = tool_args(&mut server, 37, "execute", json!({"command":"mail.get","params":{"id":message_id}})).unwrap();
            assert_eq!(filed_message["account"], destination_account);
            assert_eq!(filed_message["filingSourceAccount"], account);
            let filing_state = tool_args(&mut server, 38, "execute", json!({"command":"mail.filingStatus","params":{"id":message_id}})).unwrap();
            assert_eq!(filing_state["account"], destination_account);
            assert_eq!(filing_state["folder"], archive);
            assert_eq!(filing_state["filingSourceAccount"], account);
            assert_eq!(filing_state["serverLookupPerformed"], false);
            assert!(filing_state["recordedServer"].is_null());
            definition.as_object_mut().unwrap().remove("id");
            definition["actions"] = json!([{"type":"delete"}]);
            let denied = tool_args(&mut server, 11, "execute", json!({"command":"rules.add","params":definition})).unwrap_err();
            assert!(denied.contains("owner interaction required"), "{denied}");
            let removed = tool_args(&mut server, 35, "execute", json!({"command":"rules.remove","params":{"id":rule}})).unwrap();
            assert_eq!(removed["deleted"], rule);
            let remaining = tool_args(&mut server, 36, "execute", json!({"command":"rules.list","params":{}})).unwrap();
            assert!(remaining["rules"].as_array().unwrap().is_empty());
            let idle = tool(&mut server, 38, "status").unwrap();
            assert_eq!(idle["lifecycle"]["interactionRequired"], false);
            assert_eq!(idle["lifecycle"]["canClose"], true);
        }));
        let close = tool(&mut server, 12, "close_app").unwrap();
        assert_eq!(close["pending"], "bridgeRelease");
        drop(server);
        checked.unwrap();
    });
    let mut app = MailApp::new(session, Services::default()).with_control(requests);
    app.autosave = false;
    app.auto_sync = false;
    app.start_headless();
    pump(&mut app, &ctx, wake, None);
    client.join().unwrap();
    assert!(app.is_headless());
    assert!(app.dialog.is_none());
    assert_eq!(app.session.mailbox.message(message_id).unwrap().folder, archive);
    assert_eq!(app.session.mailbox.message(message_id).unwrap().account, destination_account);
    assert_eq!(app.session.mailbox.message(message_id).unwrap().filing_source_account, account);
    assert!(
        app.session.mailbox.queued.iter().any(|item| matches!(item, mailcraft_model::Queued::PlaceMessage {message, ..} if *message == message_id))
    );
    assert!(app.session.mailbox.rules.is_empty());
}
