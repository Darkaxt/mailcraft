// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::*;

#[test]
fn exact_rule_addresses_do_not_match_display_names_prefixes_or_substrings() {
    let sender = Condition::FromAddresses(vec!["billing@example.org".into(), "no-reply=service@example.org".into()]);
    let mut message = Message { from: Address::new("Company", "BILLING@example.org"), ..Default::default() };
    assert!(sender.matches(&message));
    for address in ["other@example.org", "billing@example.org.attacker.invalid", "prefix-billing@example.org"] {
        message.from = Address::new("billing@example.org", address);
        assert!(!sender.matches(&message));
    }
    message.from = Address::new("Company", "no-reply=service@example.org");
    assert!(sender.matches(&message));
    let recipient = Condition::ToAddresses(vec!["office@aliases.example".into()]);
    message.to = vec![Address::new("office@aliases.example", "someone@example.org")];
    assert!(!recipient.matches(&message));
    message.cc = vec![Address::new("Office", "OFFICE@aliases.example")];
    assert!(recipient.matches(&message));
    for addresses in [vec![], vec!["Company <x@example.org>".into()], vec!["x@example.org,y@example.org".into()], vec!["x@example.org\r\n".into()]] {
        assert!(Condition::FromAddresses(addresses).validate().is_err());
    }
}

#[test]
fn sender_alternatives_preserve_literal_matching_and_reject_incomplete_sources() {
    let sources = vec!["sales+news@example.org".to_string(), "İ".to_string(), "[billing]".to_string()];
    let grouped = Condition::FromAny(sources.clone());
    for name in ["[billing]", "İnvoices", "Other"] {
        for email in ["sales+news@example.org", "salesnews@example.org", "i@example.org", "nobody@example.org"] {
            let message = Message { from: Address::new(name, email), ..Default::default() };
            for addresses_only in [false, true] {
                assert_eq!(
                    grouped.matches_as(&message, addresses_only),
                    sources.iter().any(|source| Condition::FromContains(source.clone()).matches_as(&message, addresses_only))
                );
            }
        }
    }
    for invalid in [vec![], vec![String::new()], vec!["a\nb".into()], vec!["a".into(); MAX_RULE_SENDERS + 1]] {
        let condition = Condition::FromAny(invalid);
        assert!(condition.validate().is_err());
        assert!(!condition.matches(&Message::default()));
    }
}

#[test]
fn conditions_all_must_hold() {
    let m = Message {
        from: Address::new("Weekly Digest", "news@example.org"),
        to: vec![Address::new("", "me@example.org")],
        subject: "Your October digest".into(),
        attachments: vec![Attachment::new("a.pdf", "application/pdf", vec![1])],
        ..Default::default()
    };
    let mut rule = Rule {
        enabled: true,
        conditions: vec![Condition::FromContains("DIGEST".into()), Condition::HasAttachment],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    assert!(rule.matches(&m));
    rule.conditions.push(Condition::SubjectContains("invoice".into()));
    assert!(!rule.matches(&m));
    rule.conditions.pop();
    rule.enabled = false;
    assert!(!rule.matches(&m));
    // No conditions: matches nothing, not everything.
    assert!(!Rule { enabled: true, ..Default::default() }.matches(&m));
    assert!(Condition::ToContains("me@".into()).matches(&m) && !Condition::Importance(Importance::High).matches(&m));
    assert_eq!(
        Rule { conditions: vec![Condition::HasAttachment], actions: vec![Action::Flag, Action::MarkRead], ..Default::default() }.describe(),
        "When has an attachment: flag it, mark it as read"
    );
}

#[test]
fn rules_round_trip_as_json() {
    let rule = Rule {
        id: 4,
        name: "News".into(),
        enabled: true,
        conditions: vec![Condition::FromContains("news".into())],
        actions: vec![Action::MoveTo(9), Action::Categorize("Blue".into())],
        stop: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&rule).unwrap();
    assert!(json.contains(r#"{"type":"moveTo","value":9}"#), "{json}");
    assert_eq!(serde_json::from_str::<Rule>(&json).unwrap(), rule);
}

#[test]
fn local_regex_and_case_conditions_keep_unicode_hidden_text_and_invalid_exceptions_safe() {
    let m = Message { subject: "Invoice Δ42".into(), hidden_text: "Private reference ABC-123".into(), ..Default::default() };
    let mut rule: Rule = serde_json::from_value(serde_json::json!({
        "enabled": true, "conditions": [{"type": "text", "value": {
            "field": "subject", "pattern": "^invoice δ[0-9]+$", "regex": true, "caseSensitive": false}}],
        "actions": [{"type": "flag"}]
    }))
    .unwrap();
    assert!(rule.matches(&m));
    rule.conditions.push(Condition::Unless(vec![Condition::BodyContains("abc-123".into())]));
    assert!(!rule.matches(&m));
    rule.conditions.pop();
    rule.conditions.push(Condition::Unless(vec![Condition::Text(RuleText {
        field: RuleField::Body,
        pattern: "[invalid".into(),
        regex: true,
        case_sensitive: false,
    })]));
    assert!(!rule.matches(&m), "invalid exceptions must disable the entire rule rather than broadening it");
    assert!(rule.validation().is_err());
    let saved = serde_json::to_value(&rule).unwrap();
    let reopened: Rule = serde_json::from_value(saved).unwrap();
    assert_eq!(rule, reopened);
    assert!(!reopened.matches(&m));
}

#[test]
fn local_literal_matching_and_regex_compiler_limits_are_explicit() {
    let m = Message { subject: "Invoice [42]".into(), ..Default::default() };
    let mut text = RuleText { field: RuleField::Subject, pattern: "invoice [42]".into(), regex: false, case_sensitive: false };
    assert!(text.matches(&m, false));
    text.case_sensitive = true;
    assert!(!text.matches(&m, false));
    text.pattern = "Invoice [42]".into();
    assert!(text.matches(&m, false));
    text.regex = true;
    text.pattern = "(a)\\1".into();
    assert!(text.validate().is_err(), "backreferences are unsupported, never treated as literal matching");
    text.pattern = "x".repeat(4097);
    assert!(text.validate().is_err());
}

#[test]
fn exceptions_are_explicit_conditions_that_a_legacy_reader_cannot_silently_drop() {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", tag = "type", content = "value")]
    enum LegacyCondition {
        HasAttachment,
    }
    #[derive(serde::Deserialize)]
    struct LegacyRule {
        conditions: Vec<LegacyCondition>,
    }
    let rule = Rule {
        enabled: true,
        conditions: vec![Condition::HasAttachment, Condition::Unless(vec![Condition::Importance(Importance::High)])],
        actions: vec![Action::Flag],
        ..Default::default()
    };
    let saved = serde_json::to_value(&rule).unwrap();
    assert!(serde_json::from_value::<LegacyRule>(saved).is_err(), "older clients must not ignore a sibling exceptions field and broaden matching");
    let legacy = serde_json::from_value::<LegacyRule>(serde_json::json!({"conditions": [{"type": "hasAttachment"}]})).unwrap();
    assert_eq!(legacy.conditions.len(), 1);
}

#[test]
fn ordinary_unicode_word_patterns_fit_the_rule_compiler_budget() {
    let condition = RuleText { field: RuleField::From, pattern: r"^[\w.-]+@[\w.-]+\.org$".into(), regex: true, case_sensitive: false };
    assert!(condition.validate().is_ok(), "ordinary Unicode address patterns must remain usable");
    assert!(condition.matches(&Message { from: Address::new("", "billing@example.org"), ..Default::default() }, false));
}
