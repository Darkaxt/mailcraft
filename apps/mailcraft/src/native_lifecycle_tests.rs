// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit local hidden-window qualification. Synthetic mailbox/services only;
//! no preferences, credentials, real accounts, native CLI or desktop input.

use std::io;
use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use mailcraft_mcp::{Remote, Server};
use serde_json::{Value, json};
use winit::platform::windows::EventLoopBuilderExtWindows;

use super::*;

struct Fixture {
    app: MailApp,
    exited: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

impl eframe::App for Fixture {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some(window) = frame.winit_window() {
            self.app.observe_native_window(window.is_visible(), window.has_focus());
        }
        self.app.logic(ctx);
        if self.failed.load(Ordering::Acquire) {
            // Test-harness failure cleanup is a native close event, not a
            // successful MCP-action assertion or a forced process termination.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        quit::on_frame(ctx, &mut self.app);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
    }
    fn on_exit(&mut self) {
        self.exited.store(true, Ordering::Release);
    }
}

fn tool(server: &mut Server, id: usize, name: &str) -> Result<Value, String> {
    let request = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": {}}});
    let line = server.handle_line(&request.to_string()).ok_or("MCP request returned no reply")?;
    let reply: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    assert_eq!(reply["id"], id);
    assert_eq!(reply["result"]["isError"], false, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().ok_or("MCP reply has no text result")?;
    serde_json::from_str(text).map_err(|e| e.to_string())
}

#[test]
#[ignore = "explicit native hidden-window qualification; synthetic data, no desktop input"]
fn hidden_native_window_answers_mcp_and_closes_normally() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
    let key = "synthetic-lifecycle-key";
    let (started, ready) = std::sync::mpsc::channel();
    let exited = Arc::new(AtomicBool::new(false));
    let exit_receipt = exited.clone();
    let failed = Arc::new(AtomicBool::new(false));
    let client_failed = failed.clone();
    let client = std::thread::spawn(move || {
        let ctx: egui::Context = ready.recv().unwrap();
        let outcome = std::panic::catch_unwind(move || {
            let backend = Remote::connect_with_key(&address, key).unwrap().without_replay();
            let mut server = Server::new(Box::new(backend));
            let mut serial = 1;
            // Each authenticated status call wakes the UI and observes an actual new
            // native state. No sleep, timeout or replay of the hide/close actions.
            let initial = tool(&mut server, serial, "status").unwrap();
            serial += 1;
            let hide = tool(&mut server, serial, "hide_app").unwrap();
            serial += 1;
            let state = tool(&mut server, serial, "status").unwrap();
            serial += 1;
            let close = tool(&mut server, serial, "close_app").unwrap();
            println!(
                "{}",
                json!({"fixture": "synthetic native lifecycle", "startup": initial["lifecycle"], "afterHide": state["lifecycle"], "close": close})
            );
            assert_eq!(initial["lifecycle"]["hiddenRequested"], true);
            assert_eq!(initial["lifecycle"]["nativeVisible"], false);
            assert_eq!(initial["lifecycle"]["nativeFocused"], false);
            assert_eq!(hide["focusRequested"], false);
            assert_eq!(state["lifecycle"]["nativeVisible"], false);
            assert_eq!(state["lifecycle"]["nativeFocused"], false);
            assert_eq!(close["requested"], "close");
            assert_eq!(close["forced"], false);
        });
        if outcome.is_err() {
            client_failed.store(true, Ordering::Release);
            ctx.request_repaint();
        }
        outcome
    });
    let mut options = eframe::NativeOptions {
        viewport: desktop_viewport(true, true),
        persist_window: false,
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env());
    let result = eframe::run_native(
        "MailCraft hidden synthetic lifecycle",
        options,
        Box::new(move |cc| {
            let rx = control_server::serve_on(listener, cc.egui_ctx.clone(), key.into(), std::time::Duration::from_secs(30), true);
            let mut app = MailApp::new(Session::empty(), Services::default()).with_control(rx);
            app.autosave = false;
            app.start_hidden();
            started.send(cc.egui_ctx.clone()).map_err(io::Error::other)?;
            Ok(Box::new(Fixture { app, exited: exit_receipt, failed }))
        }),
    );
    result.unwrap();
    client.join().unwrap().unwrap();
    assert!(exited.load(Ordering::Acquire));
}
