// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The site a folder's icon is of ([`Session::folder_site`]): its own mail's, its parent's when it
//! has too little, its twin's in the Local archive, none for an Inbox, and worked out again once
//! mail came or went.

use mailcraft_model::{Address, FolderKind, Id, Mailbox, Message};

use crate::Session;

const NOW: i64 = 1_791_993_600;

fn mail(mb: &mut Mailbox, folder: Id, from: &str) -> Id {
    mb.add_message(Message { folder, from: Address::new("", from), date: NOW, ..Default::default() }).unwrap()
}

#[test]
fn a_folder_wears_its_mails_site_and_a_small_one_its_parents() {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let coinbase = mb.add_folder(account, None, "Coinbase").unwrap();
    let receipts = mb.add_folder(account, Some(coinbase), "Receipts").unwrap();
    let mixed = mb.add_folder(account, None, "Hotels").unwrap();
    for from in ["no-reply@info.coinbase.com", "alerts@coinbase.com", "avery@lumen-works.example", "news@coinbase.com"] {
        mail(&mut mb, coinbase, from);
    }
    mail(&mut mb, receipts, "receipts@coinbase.com");
    for from in ["a@hotel-one.example", "b@hotel-two.example", "c@hotel-three.example"] {
        mail(&mut mb, mixed, from);
    }
    for from in ["a@coinbase.com", "b@coinbase.com", "c@coinbase.com"] {
        mail(&mut mb, inbox, from);
    }
    let mut s = Session::at(mb, NOW, 0);
    assert_eq!(s.folder_site(coinbase).as_deref(), Some("coinbase.com"));
    assert_eq!(s.folder_site(receipts).as_deref(), Some("coinbase.com"), "too little of its own: its parent's");
    assert_eq!(s.folder_site(mixed), None);
    assert_eq!(s.folder_site(inbox), None, "only folders the owner made");
    // Mail came: worked out again.
    for from in ["x@hotel-one.example"; 30] {
        mail(&mut s.mailbox, mixed, from);
    }
    s.rev += 1;
    assert_eq!(s.folder_site(mixed).as_deref(), Some("hotel-one.example"));
}

#[test]
fn a_twin_in_the_local_archive_wears_its_folders_site() {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@lumen-works.example");
    let account = mb.accounts[0].id;
    let quora = mb.add_folder(account, None, "Quora").unwrap();
    for from in ["a@quora.com", "b@quora.com", "c@quora.com"] {
        mail(&mut mb, quora, from);
    }
    let twin = crate::cmd::archive::twin(&mut mb, quora).unwrap();
    let mut s = Session::at(mb, NOW, 0);
    assert_eq!(s.folder_site(twin).as_deref(), Some("quora.com"));
}
