// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search box's options and a row's relevance, as the engine states them.
use super::*;

#[test]
fn options_read_either_spelling_and_default_to_everywhere() {
    let camel = Options::from_json(&json!({"matchCase": true, "wholeWord": false, "regex": true, "searchIn": ["Body", "subject"]}));
    assert!(camel.match_case && camel.regex && !camel.whole_word);
    assert_eq!(camel.search_in, vec!["body".to_string(), "subject".to_string()]);
    let snake = Options::from_json(&json!({"match_case": false, "whole_word": true, "regex": false, "search_in": []}));
    assert!(snake.whole_word);
    assert_eq!(snake.search_in.len(), FIELDS.len(), "none named means all of them");
    assert_eq!(Options::from_json(&Value::Null), Options::default());
}

#[test]
fn whole_word_is_never_sent_with_a_regular_expression() {
    let options = Options { regex: true, whole_word: true, ..Default::default() };
    let params = options.params();
    assert_eq!(params["wholeWord"], false);
    assert_eq!(params["regex"], true);
    assert_eq!(params["searchIn"], json!(["subject", "people", "body", "attachments"]));
}

#[test]
fn a_terms_detail_says_what_it_matched_and_how_far() {
    assert_eq!(detail_line(&json!({"term": "voice", "matched": "invoice", "extra": 2})), "voice → invoice (+2 letters)");
    assert_eq!(detail_line(&json!({"term": "budget", "matched": "budget", "extra": 0, "field": "subject"})), "budget · subject");
    assert_eq!(detail_line(&json!({"term": "plans", "matched": "plan", "extra": -1})), "plans → plan (−1 letter)");
}

#[test]
fn relevance_is_offered_and_free_text_is_told_from_filters() {
    assert!(sort_choices().contains(&SortBy::Relevance));
    let mut s = Session::new(mailcraft_engine::sample::mailbox(1_791_993_600, 0));
    assert!(!free_text_active(&s));
    s.view.search = "from:ada@example.org has:attachment".into();
    assert!(!free_text_active(&s), "filters alone rank nothing");
    s.view.search = "invoice from:ada@example.org".into();
    assert!(free_text_active(&s));
}

/// A click by the pointer at `pos`.
fn click(h: &mut egui_kittest::Harness<'static, MailApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

#[test]
fn the_toggles_set_the_engines_options_and_whole_word_waits_for_regex_to_be_off() {
    let mut h = crate::people_view::testing::window("view.mail");
    h.run_steps(3);
    let at = |h: &egui_kittest::Harness<'static, MailApp>, key: &str| h.state().hit.rect(key, 0).unwrap_or_else(|| panic!("{key} in the search box"));
    let case = at(&h, "searchCase");
    click(&mut h, case.center());
    assert!(Options::of(&h.state().session).match_case);
    let regex = at(&h, "searchRegex");
    click(&mut h, regex.center());
    let options = Options::of(&h.state().session);
    assert!(options.regex && options.match_case);
    // Disabled while Regex is on: a press on it leaves it off.
    let whole = at(&h, "searchWholeWord");
    click(&mut h, whole.center());
    assert!(!Options::of(&h.state().session).whole_word);
    assert!(h.state().hit.rect("searchIn", 0).is_some());
    // Regex off: whole words can be asked for again.
    click(&mut h, regex.center());
    click(&mut h, whole.center());
    assert!(Options::of(&h.state().session).whole_word);
}
