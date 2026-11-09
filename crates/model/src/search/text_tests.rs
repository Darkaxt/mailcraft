// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exact text contracts shared by native index and in-memory search.

use super::*;

#[test]
fn quotes_preserve_punctuation_spacing_and_whole_word_edges() {
    let exact = TextTerm::new(words::ALL, "Budget review: Q4".into(), true, false).unwrap();
    assert!(exact.matches("A BUDGET REVIEW: Q4, attached."));
    for text in ["Budget review Q4", "Budget  review: Q4", "Budget review: Q40", "newBudget review: Q4", "Budget\nreview: Q4"] {
        assert!(!exact.matches(text), "{text}");
    }
    assert!(!TextTerm::new(words::ALL, "budget rev".into(), true, false).unwrap().matches("Budget review"));
    assert!(TextTerm::new(words::ALL, "预算".into(), true, false).unwrap().matches("预算案"));
    assert!(TextTerm::new(words::ALL, "@".into(), true, false).unwrap().matches("owner@example.org"));
}

#[test]
fn full_unicode_case_folding_is_distinct_from_case_sensitive_and_accent_folding() {
    let folded = TextTerm::new(words::ALL, "STRASSE".into(), true, false).unwrap();
    assert!(folded.matches("Straße"));
    assert!(TextTerm::new(words::ALL, "ΟΣ".into(), true, false).unwrap().matches("ος"));
    assert!(!TextTerm::new(words::ALL, "STRASSE".into(), true, true).unwrap().matches("Straße"));
    assert!(!TextTerm::new(words::ALL, "resume".into(), true, false).unwrap().matches("résumé"));
    assert!(TextTerm::new(words::ALL, "gvozdar".into(), false, false).unwrap().matches("Gvožđar"));
    assert!(!TextTerm::new(words::ALL, "gvozdar".into(), false, true).unwrap().matches("Gvožđar"));
}

#[test]
fn complete_prefixes_are_not_shortened_and_segment_boundaries_do_not_invent_matches() {
    let query = format!("{}final", "a".repeat(40));
    let prefix = TextTerm::new(words::ALL, query.clone(), false, false).unwrap();
    assert!(prefix.matches(&format!("{query}Suffix")));
    assert!(!prefix.matches(&format!("{}other", "a".repeat(40))));
    let prefix = TextTerm::new(words::ALL, "port".into(), false, false).unwrap();
    assert!(!prefix.matches_segment("port", false, true));
    assert!(prefix.matches_segment("port, port", false, true));
    let literal = TextTerm::new(words::ALL, "account".into(), true, false).unwrap();
    assert!(!literal.matches_segment("account", false, true));
    assert!(!literal.matches_segment("account", true, false));
    assert!(literal.matches_segment("account,", true, false));
}

#[test]
fn multiword_sources_stream_across_long_words_and_punctuation() {
    let term = TextTerm::new(words::ALL, "budget-rev".into(), false, false).unwrap();
    let mut text = term.stream();
    assert!(!text.push("bud"));
    assert!(!text.push("get"));
    assert!(!text.push(&".".repeat(100_000)));
    assert!(!text.push("re"));
    assert!(!text.push("view"));
    assert!(text.finish());
    let mut text = term.stream();
    text.push("budgets unrelated review");
    assert!(!text.finish());
}
