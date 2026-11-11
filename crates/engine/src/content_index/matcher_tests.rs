// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn options() -> SearchOptions {
    SearchOptions::default()
}

#[test]
fn substring_scores_follow_the_owner_formula() {
    let matcher = Matcher::new("voice", false, &options()).unwrap();
    assert_eq!(matcher.best("A voice message", true, true).unwrap().score, 100);
    let voices = matcher.best("Two voices", true, true).unwrap();
    assert_eq!((voices.score, voices.matched.as_str(), voices.extra), (83, "voices", 1));
    let invoice = matcher.best("Your Invoice is due", true, true).unwrap();
    assert_eq!((invoice.score, invoice.matched.as_str(), invoice.extra), (71, "Invoice", 2));
    // The best occurrence wins.
    assert_eq!(matcher.best("invoice then voice", true, true).unwrap().score, 100);
    assert!(matcher.best("no match here", true, true).is_none());
}

#[test]
fn case_accents_whole_words_and_regex() {
    let plain = Matcher::new("cafe", false, &options()).unwrap();
    assert!(plain.best("Le CAFÉ du coin", true, true).is_some(), "accent- and case-insensitive by default");
    let cased = Matcher::new("Cafe", false, &SearchOptions { match_case: true, ..options() }).unwrap();
    assert!(cased.best("le café", true, true).is_none());
    assert!(cased.best("le Café", true, true).is_some(), "matchCase keeps accents insensitive");
    let whole = Matcher::new("voice", false, &SearchOptions { whole_word: true, ..options() }).unwrap();
    assert!(whole.best("invoice", true, true).is_none());
    assert_eq!(whole.best("one voice.", true, true).unwrap().score, 100);
    assert!(whole.best("voice", false, true).is_none(), "a unit starting inside a word is not a word start");
    let regex = Matcher::new(r"inv\w+e", false, &SearchOptions { regex: true, whole_word: true, ..options() }).unwrap();
    let hit = regex.best("An INVOICE", true, true).unwrap();
    assert_eq!((hit.score, hit.matched.as_str()), (100, "INVOICE"));
    assert!(Matcher::new("(", false, &SearchOptions { regex: true, ..options() }).is_err());
}

#[test]
fn stage_one_pieces_and_regex_literals() {
    assert_eq!(pieces("Café invoice, precisely!", false), ["cafe", "invoice", "precisely"]);
    assert!(pieces("!!", false).is_empty());
    assert_eq!(regex_literals(r"inv\w+ice"), ["inv", "ice"]);
    assert_eq!(regex_literals(r"(?i)colou?r"), ["colo", "r"]);
    assert!(regex_literals("cat|dog").is_empty());
    assert!(regex_literals("(abc)?def").is_empty());
    assert_eq!(regex_literals("[abc]+xyz"), ["xyz"]);
    assert_eq!(group("voice", &["invoice".into(), "voice".into()], false).unwrap(), "(\"invoice\" OR \"voice\")");
    assert_eq!(group("a", &[], true).unwrap(), "(\"a\" *)");
    assert!(group("zzz", &[], false).is_none());
}

#[test]
fn link_fields_rank_subject_people_body_attachment() {
    assert_eq!(field_of(words::SUBJECT), ScoreField::Subject);
    assert_eq!(field_of(words::FROM), ScoreField::People);
    assert_eq!(field_of(words::BODY), ScoreField::Body);
    assert_eq!(field_of(words::HEADERS), ScoreField::Body);
    assert_eq!(field_of(words::ATTACHMENT_CONTENT), ScoreField::Attachment);
    let subject = BestHit { score: 71, matched: "Invoice".into(), extra: 2, field: ScoreField::Subject };
    let body = BestHit { field: ScoreField::Body, ..subject.clone() };
    assert!(subject.better_than(&body) && !body.better_than(&subject));
}
