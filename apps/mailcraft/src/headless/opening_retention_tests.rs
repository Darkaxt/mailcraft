// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Startup content validation owns its references until the receivers finish.

use std::sync::mpsc::channel;

use mailcraft_engine::Session;
use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ServerMessage, Tokens};
use mailcraft_model::{Body, FolderKind, Mailbox, Message, ServerFolder};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

const NOW: i64 = 1_800_000_000;

fn app() -> (MailApp, u64) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.start_headless();
    app.autosave = false;
    let account = app.execute("account.add", json!({"email": "offline@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.execute("account.edit", json!({"id": account, "keepOffline": "1w"})).unwrap();
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    app.session.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), ..Default::default() });
    let message = Message {
        account,
        folder: inbox,
        received: NOW - 30 * 86_400,
        body: Body { text: "An old message whose content is still being checked. ".repeat(100), html: None },
        server: Some(ServerMessage { id: "old".into(), folder: "in".into(), ..Default::default() }),
        ..Default::default()
    };
    let id = app.session.mailbox.add_message(message).unwrap();
    (app, id)
}

fn tick(app: &mut MailApp) {
    let (ctx, _) = super::context();
    ctx.begin_pass(egui::RawInput::default());
    app.logic(&ctx);
    let mut output = ctx.end_pass();
    output.textures_delta.clear();
}

#[test]
fn opening_reference_checks_finish_before_retention_can_remove_their_content() {
    let (mut app, id) = app();
    let (sender, receiver) = channel();
    app.check_parts_later(receiver, 0, false);
    tick(&mut app);
    assert!(!app.session.mailbox.message(id).unwrap().unfetched);
    assert!(app.session.mailbox.message(id).unwrap().body.text.len() > 1024);
    drop(sender);
    tick(&mut app);
    assert!(app.session.mailbox.message(id).unwrap().unfetched, "ordinary retention still runs after the check finishes");
}

#[test]
fn opening_body_handoff_also_finishes_before_retention_runs() {
    let (mut app, id) = app();
    let (sender, receiver) = channel();
    app.take_bodies_later(receiver);
    tick(&mut app);
    assert!(!app.session.mailbox.message(id).unwrap().unfetched);
    drop(sender);
    tick(&mut app);
    assert!(app.session.mailbox.message(id).unwrap().unfetched);
}

#[test]
fn agent_source_download_finishes_without_opening_an_owner_dialog_or_replaying() {
    let (mut app, id) = app();
    app.session.mailbox.message_mut(id).unwrap().unfetched = true;
    let account = app.session.mailbox.message(id).unwrap().account;
    app.seed_graph_token(account, Tokens { access: "synthetic".into(), expires_at: mailcraft_time::now() + 3600, ..Default::default() });
    let (requests, jobs) = channel();
    app.services.graph_fetch = Some(Box::new(move |job| {
        let (sender, receiver) = channel();
        requests.send((job, sender)).unwrap();
        receiver
    }));
    let selected = app.session.sel.messages.clone();
    let pending = json!({"pending": "download", "message": id});
    assert_eq!(app.execute("mail.viewSource", json!({"id": id})).unwrap(), pending);
    let (job, sender) = jobs.try_recv().unwrap();
    assert_eq!(job.request.ids, ["old"]);
    assert_eq!(app.execute("mail.viewSource", json!({"id": id})).unwrap(), pending);
    assert!(jobs.try_recv().is_err(), "the same in-flight request is retained");
    let other = app
        .session
        .mailbox
        .add_message(Message {
            account,
            folder: app.session.mailbox.message(id).unwrap().folder,
            unfetched: true,
            server: Some(ServerMessage { id: "other".into(), folder: "in".into(), ..Default::default() }),
            ..Default::default()
        })
        .unwrap();
    assert!(app.execute("mail.viewSource", json!({"id": other})).unwrap_err().contains("Another foreground download"));
    assert!(!job.cancel.load(std::sync::atomic::Ordering::Relaxed), "another request must not cancel the original job");
    assert!(jobs.try_recv().is_err());
    sender.send(GraphFetchResult {
        account,
        email: job.request.email,
        fetched: vec![GraphFetched { id: "old".into(), mime: b"From: source@example.org\r\nTo: offline@example.org\r\nSubject: Full source\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nComplete fetched body.\r\n".to_vec() }],
        ..Default::default()
    }).unwrap();
    app.drive_fetch();
    assert!(!app.session.mailbox.message(id).unwrap().unfetched);
    assert!(app.session.mailbox.message(id).unwrap().body.text.contains("Complete fetched body."));
    assert!(app.dialog.is_none());
    assert_eq!(app.session.sel.messages, selected);
    let source = app.execute("mail.viewSource", json!({"id": id})).unwrap();
    assert!(source["source"].as_str().unwrap().contains("Complete fetched body."));
    assert!(app.dialog.is_none());
}

#[test]
fn an_offline_agent_source_read_refuses_without_opening_a_window() {
    let (mut app, id) = app();
    app.session.mailbox.message_mut(id).unwrap().unfetched = true;
    app.execute("sync.offline", json!({"value": true})).unwrap();
    assert!(app.execute("mail.viewSource", json!({"id": id})).unwrap_err().contains("could not start"));
    assert!(app.dialog.is_none());
    assert!(app.session.mailbox.message(id).unwrap().unfetched);
}
