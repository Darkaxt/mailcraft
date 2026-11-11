// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::{FolderKind, Message};
use serde_json::json;

use super::*;

#[test]
fn the_memo_matches_a_fresh_count_and_follows_header_changes() {
    let mut session = Session::empty();
    let account = session.run("account.add", &json!({"email": "first@example.org"})).unwrap()["id"].as_u64().unwrap();
    let inbox = session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let id = session.mailbox.add_message(Message { folder: inbox, ..Default::default() }).unwrap();
    session.mailbox_changed();
    assert_eq!(session.aliases().accounts.get(&account).map_or(0, |a| a.messages_with_metadata), 0);
    // The headers arrive later (a fetch): the remembered facts are worked out again.
    let message = Arc::make_mut(session.mailbox.messages.get_mut(&id).unwrap());
    message.headers = vec![("X-SimpleLogin-Type".into(), "Forward".into()), ("X-SimpleLogin-Envelope-To".into(), "shop@aliases.example".into())];
    session.mailbox_changed();
    let inventory = session.aliases();
    assert_eq!(*inventory, Inventory::of(&session.mailbox));
    assert_eq!(inventory.accounts[&account].messages_with_metadata, 1);
    assert_eq!(inventory.accounts[&account].aliases[0].address, "shop@aliases.example");
}
