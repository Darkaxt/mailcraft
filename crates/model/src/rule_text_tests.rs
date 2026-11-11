// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bounded regex retention and refused expressions.

use super::*;

#[test]
fn many_distinct_expressions_stay_cached_and_bounded() {
    let patterns: Vec<String> = (0..64).map(|n| format!("^cache-probe-{n}-[0-9]+$")).collect();
    for _ in 0..2 {
        for (n, pattern) in patterns.iter().enumerate() {
            let message = Message { subject: format!("cache-probe-{n}-42"), ..Default::default() };
            let text = RuleText { field: RuleField::Subject, pattern: pattern.clone(), regex: true, case_sensitive: true };
            assert!(text.matches(&message, false), "{pattern}");
        }
    }
    // More than sixteen expressions remain retained instead of thrashing.
    let cache = cache();
    assert!(patterns.iter().all(|pattern| cache.entries[1].contains_key(pattern)));
    assert!(cache.entries.iter().map(HashMap::len).sum::<usize>() <= CAPACITY);
}

#[test]
fn invalid_and_oversized_expressions_never_match() {
    let message = Message { subject: "x".into(), ..Default::default() };
    for pattern in ["(".to_string(), String::new(), "x".repeat(4097)] {
        let text = RuleText { field: RuleField::Subject, pattern, regex: true, case_sensitive: false };
        assert!(text.validate().is_err());
        assert!(!text.matches(&message, false));
    }
}
