// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attachments scanned once the owner reads them: what is scanned, what is kept, what a flag
//! does, and what agents can't do with a flagged file.

use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::Attachment;
use serde_json::json;

use super::*;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Asked = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// The sample mailbox with a message carrying `names`, and a host that notes what it is asked to
/// scan and answers `answer` at once. The message's id comes back too.
fn app(names: &[&str], answer: ScanOutcome) -> (MailApp, u64, Asked) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let id = app.session.rows().first().map(|r| r.id).unwrap();
    let m = app.session.mailbox.message_mut(id).unwrap();
    m.attachments = names.iter().map(|n| Attachment::new(*n, "application/pdf", format!("bytes of {n}").into_bytes())).collect();
    let asked: Asked = Default::default();
    let sink = asked.clone();
    app.services.scan_file = Some(Box::new(move |request: ScanRequest| {
        sink.lock().unwrap().push((request.name.clone(), request.bytes.to_vec()));
        let (tx, rx) = mpsc::channel();
        tx.send(answer.clone()).unwrap();
        rx
    }));
    (app, id, asked)
}

fn flagged_answer() -> ScanOutcome {
    ScanOutcome::Flagged { threats: vec!["Trojan:PDF/Phish!MSR".to_string()] }
}

/// Let the scans waiting run to their end.
fn settle(app: &mut MailApp) {
    for _ in 0..16 {
        tick(app);
    }
}

#[test]
fn the_owner_reading_a_message_has_its_files_scanned_and_a_flag_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (mut a, id, asked) = app(&["invoice.pdf", "logo.png"], flagged_answer());
    let mailbox = dir.path().join("mailbox.mailcraft");
    a.session.path = Some(mailbox.clone());
    notice(&mut a, id);
    // Seen again at the next frame: not queued twice.
    notice(&mut a, id);
    assert_eq!(a.scans.waiting.len(), 2);
    settle(&mut a);
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "one scan each");
    assert_eq!(asked[0], ("invoice.pdf".to_string(), b"bytes of invoice.pdf".to_vec()));
    assert_eq!(flagged(&a, id, 0).as_deref(), Some("Trojan:PDF/Phish!MSR"));
    // Kept beside the mailbox, by content, and read back by another run.
    let kept = read(&mailbox);
    let hash = hash_of(&a.session.mailbox.message(id).unwrap().attachments[0]).unwrap();
    assert_eq!(kept.get(&hash).map(|k| (k.verdict, k.at, k.engine.as_str())), Some((Verdict::Flagged, NOW, ENGINE)));
    // Nothing of the message changed.
    assert_eq!(a.session.mailbox.message(id).unwrap().attachments.len(), 2);
    // Read again: a flag holds, nothing is scanned again.
    notice(&mut a, id);
    assert!(a.scans.waiting.is_empty());
}

#[test]
fn the_sidecar_round_trips_and_a_clean_verdict_expires_after_a_week() {
    let dir = tempfile::tempdir().unwrap();
    let mailbox = dir.path().join("mailbox.mailcraft");
    assert_eq!(sidecar(&mailbox), dir.path().join("mailbox.mailcraft.scans.json"));
    let (mut a, id, asked) = app(&["fresh.pdf", "stale.pdf", "bad.pdf"], ScanOutcome::Clean);
    let files = &a.session.mailbox.message(id).unwrap().attachments;
    let (fresh, stale, bad) = (hash_of(&files[0]).unwrap(), hash_of(&files[1]).unwrap(), hash_of(&files[2]).unwrap());
    let mut known = BTreeMap::new();
    known.insert(fresh, Known { verdict: Verdict::Clean, threats: vec![], at: NOW - 60, engine: ENGINE.into() });
    known.insert(stale.clone(), Known { verdict: Verdict::Clean, threats: vec![], at: NOW - CLEAN_FOR - 1, engine: ENGINE.into() });
    known.insert(bad, Known { verdict: Verdict::Flagged, threats: vec!["X".into()], at: NOW - 100 * CLEAN_FOR, engine: ENGINE.into() });
    // Junk among them is left out.
    known.insert("not a hash".into(), Known { verdict: Verdict::Clean, threats: vec![], at: NOW, engine: ENGINE.into() });
    write(&mailbox, &known).unwrap();
    let back = read(&mailbox);
    assert_eq!(back.len(), 3);
    assert!(!dir.path().join("mailbox.mailcraft.scans.json.tmp").exists());
    a.session.path = Some(mailbox.clone());
    notice(&mut a, id);
    // Only the clean verdict older than a week is scanned again; the flag stays.
    assert_eq!(a.scans.waiting.iter().map(|w| w.hash.clone()).collect::<Vec<_>>(), vec![stale.clone()]);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert_eq!(read(&mailbox).get(&stale).map(|k| k.at), Some(NOW));
    assert_eq!(flagged(&a, id, 2).as_deref(), Some("X"));
    // A file that isn't a verdicts file reads as none.
    std::fs::write(sidecar(&mailbox), b"{not json").unwrap();
    assert!(read(&mailbox).is_empty());
}

#[test]
fn a_failed_scan_is_not_kept_or_retried_by_itself() {
    let (mut a, id, asked) = app(&["report.pdf"], ScanOutcome::Failed { why: "no".into() });
    notice(&mut a, id);
    settle(&mut a);
    notice(&mut a, id);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(a.scans.known.is_empty());
    assert!(flagged(&a, id, 0).is_none());
}

#[test]
fn an_agent_reading_a_message_scans_nothing_and_is_told_what_is_known() {
    let (mut a, id, asked) = app(&["invoice.pdf"], flagged_answer());
    a.session.agent = Some(vec![]);
    let got = a.execute("mail.get", json!({"id": id})).unwrap();
    a.session.agent = None;
    settle(&mut a);
    assert!(asked.lock().unwrap().is_empty());
    assert!(got["attachments"][0].get("scan").is_none());
    // Once the owner read it, `mail.get` says what was found.
    notice(&mut a, id);
    settle(&mut a);
    let got = a.execute("mail.get", json!({"id": id})).unwrap();
    assert_eq!(got["attachments"][0]["scan"]["verdict"], "flagged");
    assert_eq!(got["attachments"][0]["scan"]["threats"][0], "Trojan:PDF/Phish!MSR");
}

#[test]
fn headless_and_hosts_without_a_scanner_scan_nothing() {
    let (mut a, id, asked) = app(&["invoice.pdf"], flagged_answer());
    a.start_headless();
    notice(&mut a, id);
    settle(&mut a);
    assert!(asked.lock().unwrap().is_empty());
    assert!(a.execute(SCAN, json!({"id": id, "index": 0})).is_err());
    let (mut a, id, _) = app(&["invoice.pdf"], flagged_answer());
    a.services.scan_file = None;
    notice(&mut a, id);
    assert!(a.execute(SCAN, json!({"id": id, "index": 0})).is_err());
}

#[test]
fn opening_a_flagged_file_asks_first() {
    let (mut a, id, _) = app(&["invoice.pdf"], flagged_answer());
    notice(&mut a, id);
    settle(&mut a);
    let opened: Asked = Default::default();
    let sink = opened.clone();
    a.services.open_file = Some(Box::new(move |name, bytes| {
        sink.lock().unwrap().push((name.to_string(), bytes.to_vec()));
        Ok(())
    }));
    a.run("mail.select", json!({"id": id})).unwrap();
    let r = a.run("mail.openAttachment", json!({"id": id, "index": 0})).unwrap();
    assert_eq!(r["pending"], "confirm");
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("no question") };
    assert_eq!((ask.title.as_str(), ask.action.as_str()), ("Opening Mail Attachment", "Open anyway"));
    assert!(ask.text.contains("Trojan:PDF/Phish!MSR"), "{}", ask.text);
    a.dialog = None;
    assert!(opened.lock().unwrap().is_empty());
    a.run("mail.openAttachment", json!({"id": id, "index": 0, "confirmed": true})).unwrap();
    assert_eq!(opened.lock().unwrap().len(), 1);
}

#[test]
fn saving_a_flagged_file_asks_the_owner_and_is_refused_to_agents() {
    let (mut a, id, _) = app(&["invoice.pdf", "notes.txt"], flagged_answer());
    // Only the first is flagged.
    notice(&mut a, id);
    a.scans.waiting.truncate(1);
    settle(&mut a);
    assert!(flagged(&a, id, 1).is_none());
    a.run("mail.select", json!({"id": id})).unwrap();
    let r = a.run("mail.saveAttachment", json!({"id": id, "index": 0})).unwrap();
    assert_eq!(r["pending"], "confirm");
    let Some(crate::dialogs::Dialog::Confirm { ask, params, .. }) = &a.dialog else { panic!("no question") };
    assert_eq!((ask.title.as_str(), ask.action.as_str()), ("Saving Mail Attachment", "Save anyway"));
    assert_eq!(params["id"], id);
    a.dialog = None;
    assert!(a.run("mail.saveAllAttachments", json!({"id": id})).unwrap()["pending"] == "confirm");
    a.dialog = None;
    // The file that wasn't flagged asks nothing.
    assert!(save_question(&a, "mail.saveAttachment", &json!({"id": id, "index": 1})).is_none());
    // Agents: refused, naming the threat.
    let why = agent_refusal(&a, "mail.saveAttachment", &json!({"id": id, "index": 0})).unwrap();
    assert!(why.contains("Trojan:PDF/Phish!MSR") && why.contains("invoice.pdf"), "{why}");
    assert!(agent_refusal(&a, "mail.saveAllAttachments", &json!({"id": id})).is_some());
    assert!(agent_refusal(&a, "mail.saveAttachment", &json!({"id": id, "index": 1})).is_none());
    assert!(agent_refusal(&a, "mail.get", &json!({"id": id})).is_none());
}

#[test]
fn scan_attachment_queues_a_fresh_scan() {
    let (mut a, id, asked) = app(&["invoice.pdf"], ScanOutcome::Clean);
    notice(&mut a, id);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 1);
    // Clean and current: reading it again scans nothing, asking does.
    notice(&mut a, id);
    assert!(a.scans.waiting.is_empty());
    let r = a.execute(SCAN, json!({"id": id, "index": 0})).unwrap();
    assert_eq!(r["queued"], true);
    assert_eq!(r["hash"].as_str().map(str::len), Some(64));
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().len(), 2);
    // Agents may ask for it: it only reports.
    assert_eq!(mailcraft_engine::agent::access(SCAN), Some(mailcraft_engine::agent::Access::Safe));
    // Junk is refused, not crashed on.
    for p in [json!({}), json!({"id": id, "index": 9}), json!({"id": "x", "index": -1}), json!(null)] {
        assert!(a.execute(SCAN, p).is_err());
    }
}
