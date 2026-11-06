// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The native transport commits close only after writing the accepted reply.

use super::*;

#[test]
fn a_close_receipt_is_native_only_and_follows_the_reply_write() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let key = "synthetic-control-key";
    let rx = serve_on(listener, egui::Context::default(), key.into(), FIRST_REQUEST, true);
    let client = std::thread::spawn(move || {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        writeln!(socket, "{}", json!({"id": "close-once", "key": key, "method": "app.lifecycle", "params": {"action": "close"}, "agent": {"allow": ["input"]}, "close_receipt": true})).unwrap();
        let mut reply = String::new();
        BufReader::new(&mut socket).read_line(&mut reply).unwrap();
        (serde_json::from_str::<Value>(&reply).unwrap(), socket)
    });
    let request = rx.recv().unwrap();
    assert_eq!(request.agent, Some(Vec::new()));
    let receipt = request.close_receipt.as_ref().unwrap();
    assert_eq!(receipt.outcome(), None);
    request.reply.send(json!({"ok": true, "result": {"requested": "close"}})).unwrap();
    let (reply, mut barrier) = client.join().unwrap();
    assert_eq!(reply["id"], "close-once");
    assert_eq!(reply["ok"], true);
    // The receipt is completed on the server thread after the write, independently
    // of client scheduling. A second request on the same connection is an event
    // barrier through that same server thread, with no sleep or deadline.
    writeln!(barrier, "{}", json!({"id": "barrier", "key": key, "method": "app.status"})).unwrap();
    let next = rx.recv().unwrap();
    assert_eq!(next.method, "app.status");
    assert!(next.close_receipt.is_none());
    assert_eq!(receipt.delivery(), Some(true));
    assert_eq!(receipt.outcome(), None, "the bridge must release before actual close");
    next.reply.send(json!({"ok": true, "result": {}})).unwrap();
}
