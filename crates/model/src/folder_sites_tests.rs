// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder's site: one brand's mail, several countries of it, mixed mail, too little, the owner's
//! own replies, people at mail providers, and SimpleLogin's forwards.

use std::collections::HashSet;

use crate::folder_sites::{Verdict, brand, sender_site, verdict, wears_an_icon};
use crate::{Address, Message};

fn from(address: &str) -> Message {
    Message { from: Address::new("", address), ..Default::default() }
}

fn of(addresses: &[&str]) -> Verdict {
    let own: HashSet<String> = ["avery@lumen-works.example".to_string()].into();
    let mail: Vec<Message> = addresses.iter().map(|a| from(a)).collect();
    verdict(&mail, &own)
}

#[test]
fn a_folder_of_one_companys_mail_wears_its_site() {
    assert_eq!(of(&["no-reply@info.coinbase.com", "Alerts@Coinbase.com", "news@mail.coinbase.com"]), Verdict::Site("coinbase.com".into()));
    // The site under a country's suffix.
    assert_eq!(of(&["a@news.bbc.co.uk", "b@bbc.co.uk", "c@bbc.co.uk"]), Verdict::Site("bbc.co.uk".into()));
}

#[test]
fn one_brands_countries_count_as_one() {
    let mail = ["a@amazon.es", "b@amazon.es", "c@amazon.es", "d@amazon.com", "e@amazon.de", "f@marketplace.amazon.co.uk"];
    assert_eq!(of(&mail), Verdict::Site("amazon.es".into()));
}

#[test]
fn nine_in_ten_or_no_icon() {
    let mut mail = vec!["x@patreon.com"; 9];
    mail.push("y@other.example");
    assert_eq!(of(&mail), Verdict::Site("patreon.com".into()));
    mail.push("z@third.example");
    assert_eq!(of(&mail), Verdict::Mixed);
}

#[test]
fn too_little_to_tell_and_the_owners_replies_dont_count() {
    assert_eq!(of(&[]), Verdict::TooFew);
    assert_eq!(of(&["a@quora.com", "b@quora.com"]), Verdict::TooFew);
    assert_eq!(of(&["a@quora.com", "b@quora.com", "Avery@Lumen-Works.example", "avery@lumen-works.example"]), Verdict::TooFew);
    assert_eq!(of(&["a@quora.com", "b@quora.com", "c@quora.com", "avery@lumen-works.example"]), Verdict::Site("quora.com".into()));
}

#[test]
fn people_at_a_mail_provider_wear_no_icon() {
    assert_eq!(of(&["sam@gmail.com", "kim@gmail.com", "lee@gmail.com"]), Verdict::Mixed);
    assert!(!wears_an_icon("gmail.com") && !wears_an_icon("co.uk") && !wears_an_icon("contoso.onmicrosoft.com"));
    assert!(wears_an_icon("coinbase.com"));
    assert_eq!(brand("amazon.co.uk"), "amazon");
}

#[test]
fn simplelogins_forward_counts_as_from_its_original_sender() {
    let own = HashSet::new();
    let mut m = from("broker_at_ibkr_com_abc@simplelogin.co");
    m.to = vec![Address::new("", "me.alias@aleeas.com")];
    m.headers = vec![
        ("X-SimpleLogin-Type".into(), "Forward".into()),
        ("X-SimpleLogin-Envelope-To".into(), "me.alias@aleeas.com".into()),
        ("X-SimpleLogin-Original-From".into(), "Interactive Brokers <donotreply@interactivebrokers.com>".into()),
        ("Received".into(), "from mail-200.simplelogin.co (mail-200.simplelogin.co [192.0.2.10]) by mx.aliases.example".into()),
    ];
    assert_eq!(sender_site(&m, &own).as_deref(), Some("interactivebrokers.com"));
    assert_eq!(sender_site(&from("a@simplelogin.co"), &own), None);
    assert_eq!(sender_site(&from("not an address"), &own), None);
}
