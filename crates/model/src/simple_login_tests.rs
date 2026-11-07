// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forwarding metadata never replaces transport identity or authenticates a sender.

use crate::{Address, Message};

fn forwarded() -> Message {
    Message {
        from: Address::new("Reported name", "contact_random@simplelogin.co"),
        reply_to: vec![Address::new("Reply route", "reply_random@simplelogin.co")],
        to: vec![Address::new("", "shop@aliases.example")],
        headers: vec![
            ("X-SimpleLogin-Type".into(), "Forward".into()),
            ("X-SimpleLogin-Envelope-To".into(), "shop@aliases.example".into()),
            ("X-SimpleLogin-Original-From".into(), "Shop Team <sender@shop.example>".into()),
            ("X-SimpleLogin-Envelope-From".into(), "bounce@shop.example".into()),
        ],
        ..Default::default()
    }
}

#[test]
fn complete_header_claim_has_separate_alias_original_and_envelope_sender() {
    let message = forwarded();
    let identity = message.simple_login().unwrap();
    assert_eq!(identity.receiving_alias.as_deref(), Some("shop@aliases.example"));
    assert_eq!(identity.original_from, Some(Address::new("Shop Team", "sender@shop.example")));
    assert_eq!(identity.envelope_from.as_deref(), Some("bounce@shop.example"));
    assert!(identity.issues.is_empty());
    assert_eq!(message.from.email, "contact_random@simplelogin.co");
    assert_eq!(message.reply_to[0].email, "reply_random@simplelogin.co");
    let value = serde_json::to_value(&identity).unwrap();
    assert_eq!(value["provenance"], "messageHeaders");
    assert_eq!(value["authenticated"], false);
}

#[test]
fn missing_original_is_an_explicit_gap_and_never_decoded_from_the_reverse_alias() {
    let mut message = forwarded();
    message.headers.retain(|(name, _)| name != "X-SimpleLogin-Original-From");
    message.from = Address::new("sender at shop.example", "sender_at_shop_example_random@simplelogin.co");
    let identity = message.simple_login().unwrap();
    assert_eq!(identity.receiving_alias.as_deref(), Some("shop@aliases.example"));
    assert!(identity.original_from.is_none());
    assert!(identity.issues.iter().any(|issue| issue.field == "originalFrom" && issue.reason == "missing"));
}

#[test]
fn duplicate_or_hostile_values_are_not_silently_chosen_or_truncated() {
    for (name, value) in [
        ("X-SimpleLogin-Envelope-To", "other@aliases.example"),
        ("X-SimpleLogin-Envelope-To", "shop@aliases.example"),
        ("X-SimpleLogin-Original-From", "Other <other@shop.example>"),
    ] {
        let mut message = forwarded();
        message.headers.push((name.into(), value.into()));
        let identity = message.simple_login().unwrap();
        assert!(identity.issues.iter().any(|issue| issue.reason == "duplicate"));
        if name.ends_with("To") {
            assert!(identity.receiving_alias.is_none());
        } else {
            assert!(identity.original_from.is_none());
        }
    }
    for value in [
        "A <one@shop.example>, B <two@shop.example>",
        "A <one@shop.example> ignored",
        "one@shop.example\r\nBcc: victim@example.org",
        "not-an-address",
    ] {
        let mut message = forwarded();
        message.headers[2].1 = value.into();
        let identity = message.simple_login().unwrap();
        assert!(identity.original_from.is_none(), "{value}");
        assert!(identity.issues.iter().any(|issue| issue.field == "originalFrom" && issue.reason == "invalid"));
    }
}

#[test]
fn only_forward_markers_give_alias_membership_and_headers_survive_roundtrip() {
    assert!(Message::default().simple_login().is_none());
    for marker in ["Reply", "Unknown", ""] {
        let mut message = forwarded();
        message.headers[0].1 = marker.into();
        let identity = message.simple_login().unwrap();
        assert!(identity.receiving_alias.is_none());
        assert!(identity.issues.iter().any(|issue| issue.field == "direction"));
    }
    let message = forwarded();
    let saved = serde_json::to_vec(&message).unwrap();
    let reopened: Message = serde_json::from_slice(&saved).unwrap();
    assert_eq!(message.simple_login(), reopened.simple_login());
    assert_eq!(message.from, reopened.from);
    assert_eq!(message.reply_to, reopened.reply_to);
}

#[test]
fn explicit_search_and_rule_fields_do_not_reinterpret_transport_matching() {
    use crate::{RuleField, RuleText, search::Query};
    let message = forwarded();
    for query in ["alias:SHOP@aliases.example", "originalfrom:sender@shop.example", "transportfrom:contact_random@simplelogin.co"] {
        assert!(Query::parse(query).matches(&message, 0), "{query}");
    }
    for query in ["alias:other@aliases.example", "alias:shop", "originalfrom:bounce@shop.example", "from:sender@shop.example"] {
        assert!(!Query::parse(query).matches(&message, 0), "{query}");
    }
    assert!(Query::parse("alias:shop@aliases.example").for_server().terms.is_empty());
    assert!(Query::parse("originalfrom:sender@shop.example").for_server().terms.is_empty());
    for (field, pattern) in [(RuleField::OriginalFrom, "^sender@shop\\.example$"), (RuleField::ReceivingAlias, "^shop@aliases\\.example$")] {
        let rule = RuleText { field, pattern: pattern.into(), regex: true, case_sensitive: true };
        assert!(rule.matches(&message, true));
        let serialized = serde_json::to_vec(&rule).unwrap();
        assert_eq!(serde_json::from_slice::<RuleText>(&serialized).unwrap(), rule);
    }
}
