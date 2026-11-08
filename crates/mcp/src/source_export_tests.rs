// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! New source pages remain untrusted text; MIME chunks remain decodable bytes.

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::{
    Headless,
    tools::{Policy, UNTRUSTED_CLOSE, UNTRUSTED_OPEN, call_tool},
};

fn execute(host: &mut Headless, command: &str, params: Value) -> Value {
    let result = call_tool(host, &Policy::default(), "execute", &json!({"command":command,"params":params}));
    assert!(!result.is_error, "{:?}", result.content);
    serde_json::from_str(result.content[0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn source_is_framed_as_hostile_text_and_eml_data_is_not_rewritten() {
    let session = Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0);
    let id = *session.mailbox.messages.keys().next().unwrap();
    let mut host = Headless::new(session, Vec::new());
    let legacy = execute(&mut host, "mail.viewSource", json!({"id":id}));
    assert!(legacy["source"].as_str().unwrap().starts_with(UNTRUSTED_OPEN));
    for command in ["mail.sourcePage", "mail.exportEml"] {
        let opened = execute(&mut host, command, json!({"id":id}));
        let token = opened["token"].as_u64().unwrap();
        host.session.wait_source_exports();
        let page = execute(&mut host, command, json!({"id":id,"token":token}));
        if command == "mail.sourcePage" {
            let source = page["source"].as_str().unwrap();
            assert!(source.starts_with(UNTRUSTED_OPEN));
            assert!(source.ends_with(UNTRUSTED_CLOSE));
        } else {
            let bytes = mailcraft_engine::cmd::base64_decode(page["data"].as_str().unwrap()).unwrap();
            assert!(bytes.starts_with(b"Date:"));
            assert_eq!(page["originalMimeAvailable"], false);
        }
        assert_eq!(execute(&mut host, command, json!({"id":id,"token":token,"close":true}))["state"], "closed");
    }
}
