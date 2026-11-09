// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finite offline mode reuses the actual maintained index; no CLI child, live
//! mailbox, account sign-in, GUI or server is started by these unit workflows.

use mailcraft_model::{Body, Mailbox, Message};

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).into()).collect()
}

#[test]
fn offline_index_mode_requires_an_explicit_file() {
    let error = super::run(&args(&["run", "--index", "--sample"])).unwrap_err().to_string();
    assert!(error.contains("--index requires an explicit offline --file"));
}

#[test]
fn finite_offline_commands_persist_the_index_and_observe_original_completion() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("offline.mailcraft");
    let mut mailbox = Mailbox::with_local_account("Avery", "avery@example.org");
    let folder = mailbox.inbox().unwrap();
    mailbox
        .add_message(Message {
            folder,
            account: mailbox.accounts[0].id,
            subject: "Offline source".into(),
            body: Body { text: "PersistedCobalt".into(), html: None },
            ..Default::default()
        })
        .unwrap();
    mailcraft_engine::store::save(&path, &mailbox).unwrap();
    super::run(&args(&[
        "run",
        "--file",
        path.to_str().unwrap(),
        "--index",
        "--cmd",
        "view.search={\"query\":\"PersistedCobalt\"}",
        "--cmd",
        "mail.list={\"limit\":10}",
    ]))
    .unwrap();
    let index = mailcraft_engine::store::content_index_file(&path);
    assert!(index.is_file());
    let mut session = mailcraft_engine::Session::new(mailcraft_engine::store::open(&path).unwrap().mailbox);
    session.path = Some(path);
    session.index_words = true;
    session.view.search = "PersistedCobalt".into();
    session.wait_for_word_index();
    assert_eq!(session.rows().len(), 1);
    assert!(session.content_index_status()["complete"].as_bool().unwrap());
}
