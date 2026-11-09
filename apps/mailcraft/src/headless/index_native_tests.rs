// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whole native index acceptance through the authenticated windowless app.
//! Explicit inert inputs only; no candidate CLI, live mailbox or desktop input.

use std::net::TcpListener;
use std::path::Path;

use mailcraft_engine::Session;
use mailcraft_mcp::{Remote, Server};
use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::{Value, json};

fn execute(server: &mut Server, id: &mut usize, command: &str, params: Value) -> Result<Value, String> {
    *id += 1;
    super::headless_tests::tool_args(server, *id, "execute", json!({"command":command,"params":params}))
}

fn settled(server: &mut Server, id: &mut usize) -> Result<Value, String> {
    loop {
        let state = execute(server, id, "search.indexStatus", json!({}))?;
        assert!(state["problem"].is_null(), "{state}");
        if state["initialized"] == true
            && state["building"] == false
            && state["metadataPending"] == 0
            && state["bodiesPending"] == 0
            && state["attachmentsPending"] == 0
            && state["coverageCurrent"] == true
        {
            return Ok(state);
        }
        // Every request observes the original native/index workflow. No deadline,
        // cancellation, replay, replacement process or artificial delay.
    }
}

fn quoted(text: &str) -> String {
    format!("attachmenttext:\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn evidence(server: &mut Server, request_id: &mut usize, document: u64, query: &str) -> Result<Value, String> {
    let mut params = json!({"id":document,"query":query,"limit":100});
    let first = execute(server, request_id, "search.sources", params.clone())?;
    params["token"] = first["token"].clone();
    assert!(params["token"].as_u64().is_some(), "{first}");
    let answer = loop {
        let answer = execute(server, request_id, "search.sources", params.clone())?;
        if answer["state"] == "ready" {
            break answer;
        }
        assert!(matches!(answer["state"].as_str(), Some("pending" | "running")), "{answer}");
    };
    assert!(answer["result"]["totalSources"].as_u64().is_some_and(|count| count > 0), "{query}: {answer}");
    assert!(
        answer["result"]["sources"].as_array().is_some_and(|sources| sources.iter().all(|source| source["source"]["attachment"] == 0)),
        "{answer}"
    );
    params["close"] = json!(true);
    assert_eq!(execute(server, request_id, "search.sources", params)?["state"], "closed");
    Ok(answer["result"].clone())
}

fn coverage_details(server: &mut Server, request_id: &mut usize, document: u64) -> Result<Value, String> {
    let mut sections = serde_json::Map::new();
    for section in ["gaps", "sources"] {
        let mut offset = 0;
        let mut pages = Vec::new();
        loop {
            let mut params = json!({"id":document,"attachment":0,"section":section,"offset":offset,"limit":100});
            let first = execute(server, request_id, "search.coverageDetails", params.clone())?;
            params["token"] = first["token"].clone();
            assert!(params["token"].as_u64().is_some(), "{first}");
            let answer = loop {
                let answer = execute(server, request_id, "search.coverageDetails", params.clone())?;
                if answer["state"] == "ready" {
                    break answer["result"].clone();
                }
                assert!(matches!(answer["state"].as_str(), Some("pending" | "running")), "{answer}");
            };
            params["close"] = json!(true);
            assert_eq!(execute(server, request_id, "search.coverageDetails", params)?["state"], "closed");
            let next = answer["nextOffset"].as_u64();
            pages.push(answer);
            match next {
                Some(next) => {
                    assert!(next > offset, "the original native coverage page did not advance");
                    offset = next;
                }
                None => break,
            }
        }
        sections.insert(section.into(), json!(pages));
    }
    Ok(Value::Object(sections))
}

#[test]
#[ignore = "whole-stage native payload qualification; explicit MAILCRAFT_INDEX_NATIVE_FIXTURES and MAILCRAFT_INDEX_NATIVE_WORK required"]
fn native_index_acceptance_through_headless_mcp_and_reopen() {
    let fixtures = std::path::PathBuf::from(std::env::var("MAILCRAFT_INDEX_NATIVE_FIXTURES").unwrap());
    let work = std::path::PathBuf::from(std::env::var("MAILCRAFT_INDEX_NATIVE_WORK").unwrap());
    assert!(fixtures.is_absolute() && work.is_absolute() && work.is_dir());
    let manifest: Value = serde_json::from_slice(&std::fs::read(fixtures.join("manifest.json")).unwrap()).unwrap();
    let records = manifest["fixtures"].as_array().unwrap();
    assert_eq!(records.len(), 8);
    let path = work.join("native-index.mailcraft");
    assert!(!path.exists(), "preserve the original qualification mailbox");
    let mut mailbox = Mailbox::with_local_account("Native qualification", "fixtures@example.org");
    let folder = mailbox.inbox().unwrap();
    let account = mailbox.accounts[0].id;
    let mut cases = Vec::new();
    for record in records {
        let name = record["file"].as_str().unwrap();
        assert_eq!(Path::new(name).components().count(), 1);
        let bytes = std::fs::read(fixtures.join(name)).unwrap();
        let sha: String = ring::digest::digest(&ring::digest::SHA256, &bytes).as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(sha, record["sha256"]);
        let document = mailbox
            .add_message(Message {
                account,
                folder,
                from: Address::new("Inert fixtures", "fixtures@example.net"),
                subject: format!("Native fixture {}", record["engine"].as_str().unwrap()),
                body: Body { text: "Expected search words occur only in the attachment.".into(), html: None },
                attachments: vec![Attachment::new(name, record["mime"].as_str().unwrap(), bytes)],
                ..Default::default()
            })
            .unwrap();
        let mut expected = if let Some(text) = record["expectedNative"].as_str() {
            vec![text.to_owned()]
        } else {
            record["expectedOcr"].as_array().unwrap().iter().map(|item| item["text"].as_str().unwrap().to_owned()).collect()
        };
        if let Some(changes) = record["changes"].as_array() {
            expected.extend(changes.iter().filter_map(|change| change["cachedValue"].as_str().map(str::to_owned)));
        }
        cases.push((document, record["engine"].clone(), sha, expected));
    }
    mailcraft_engine::store::save(&path, &mailbox).unwrap();
    drop(mailbox);
    let captures = work.join("renders");
    std::fs::create_dir(&captures).unwrap();
    let mut rounds = Vec::new();
    for round in 0..2 {
        let opened = mailcraft_engine::store::open(&path).unwrap();
        let mut session = Session::new(opened.mailbox);
        session.path = Some(path.clone());
        session.index_words = true;
        session.prefs.offline = true;
        if round == 0 {
            session.index_render_evidence = Some(captures.clone());
        }
        let (ctx, wake) = super::context();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let key = "synthetic-native-index-key";
        let requests = crate::control_server::serve_on(listener, ctx.clone(), key.into(), std::time::Duration::from_secs(30), true);
        let backend = Remote::connect_with_key(&address, key).unwrap().without_replay();
        let cases = cases.clone();
        let client = std::thread::spawn(move || {
            let mut server = Server::new(Box::new(backend));
            let mut request_id = 100;
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Value, String> {
                let status = settled(&mut server, &mut request_id)?;
                let mut matches = Vec::new();
                let mut coverage = Vec::new();
                for (document, engine, sha, expected) in cases {
                    coverage.push(json!({"engine":engine,"document":document,
                        "details":coverage_details(&mut server,&mut request_id,document)?}));
                    for text in expected {
                        let query = quoted(&text);
                        let listed = loop {
                            let listed = execute(&mut server, &mut request_id, "mail.list", json!({"query":query,"limit":100}))?;
                            assert!(listed["searchIndex"]["problem"].is_null(), "{listed}");
                            if listed["searchIndex"]["queryPending"] == false {
                                break listed;
                            }
                        };
                        assert!(listed["messages"].as_array().unwrap().iter().any(|row| row["id"] == document), "{engine}: {text}: {listed}");
                        matches.push(json!({"engine":engine,"document":document,"sha256":sha,"query":query,
                            "sources":evidence(&mut server,&mut request_id,document,&query)?}));
                    }
                }
                Ok(json!({"round":round,"coverage":status,"attachmentCoverage":coverage,"matches":matches}))
            }));
            request_id += 1;
            let close = super::headless_tests::tool_args(&mut server, request_id, "close_app", json!({})).unwrap();
            assert_eq!(close["pending"], "bridgeRelease");
            drop(server);
            checked.unwrap().unwrap()
        });
        let mut app = MailApp::new(session, Services::default()).with_control(requests);
        app.auto_sync = false;
        app.autosave = false;
        app.start_headless();
        super::pump(&mut app, &ctx, wake, None, None);
        rounds.push(client.join().unwrap());
        assert!(app.is_headless() && app.dialog.is_none());
        assert!(app.session.mailbox.queued.is_empty());
        drop(app);
    }
    assert!(captures.join("renders.jsonl").is_file());
    std::fs::write(
        work.join("native-index-qualification.json"),
        serde_json::to_vec_pretty(&json!({"rounds":rounds,"nativeWindow":false,"liveAccounts":false})).unwrap(),
    )
    .unwrap();
}
