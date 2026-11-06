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
    let line = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": {}}});
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
        }));
        let close = tool(&mut server, 3, "close_app").unwrap();
        assert_eq!(close["pending"], "bridgeRelease");
        drop(server);
        checked.unwrap();
    });
    let mut app = MailApp::new(Session::new(mailcraft_engine::sample::mailbox(0, 0)), Services::default()).with_control(requests);
    app.autosave = false;
    app.auto_sync = false;
    app.start_headless();
    pump(&mut app, &ctx, wake, None);
    client.join().unwrap();
    assert!(app.is_headless());
    assert!(app.dialog.is_none());
}
