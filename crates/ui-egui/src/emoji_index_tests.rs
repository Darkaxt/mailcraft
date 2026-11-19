// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn all() -> Vec<&'static Emoji> {
    emojis::iter().filter(|e| offered(e, |_| true)).collect()
}

/// A search goes by the words of an emoji's name or a short code, in any order; one typed or
/// pasted in finds itself; nothing typed finds nothing.
#[test]
fn a_search_finds_emoji_by_their_names() {
    let list = all();
    let found = |q: &str| search(&list, q).into_iter().map(|(s, _)| s).collect::<Vec<_>>();
    assert!(found("house").contains(&"🏠"));
    // (Written out: theme.rs wants a glyph for each symbol in the source, and the picture
    // selector and the white flag have none.)
    assert!(found("heart red").contains(&"\u{2764}\u{FE0F}"), "{:?}", found("heart red"));
    assert!(found("Thumbs").contains(&"👍"), "case doesn't matter");
    assert!(found("tada").contains(&"🎉"), "a short code");
    assert_eq!(found("🏦"), ["🏦"]);
    assert!(found("  ").is_empty() && found("no such emoji at all").is_empty());
}

/// The index offers far more than the suggested few, every group has some, and each fits in a tag.
#[test]
fn every_group_is_offered_and_each_emoji_fits_a_tag() {
    let list = all();
    assert!(list.len() > 1000, "{}", list.len());
    for (tab, face, _) in TABS {
        let shown = in_tab(&list, *tab);
        assert!(!shown.is_empty(), "{tab:?}");
        assert!(emojis::get(face).is_some(), "{face} names its tab");
    }
    assert_eq!(in_tab(&list, Tab::Suggested).len(), SUGGESTED.len());
    assert!(in_tab(&list, Tab::Suggested).iter().all(|(_, name)| !name.is_empty()), "every suggestion has its name");
    assert!(list.iter().all(|e| e.as_str().chars().count() <= MAX_TAG));
}

/// What the fonts can't draw isn't offered, and on Windows nor are the flags of places, which
/// would show as two letters; the chequered flag is.
#[test]
fn what_cant_be_drawn_is_left_out() {
    let house = emojis::get("🏠").unwrap();
    assert!(offered(house, |_| true) && !offered(house, |c| c != '🏠'));
    // The joiner and the picture selector need no glyph of their own.
    let rainbow = emojis::get("\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}").unwrap();
    assert!(offered(rainbow, |c| !matches!(c, '\u{200D}' | '\u{FE0F}')));
    let spain = emojis::get("🇪🇸").unwrap();
    assert_eq!(offered(spain, |_| true), !cfg!(windows));
    assert!(offered(emojis::get("🏁").unwrap(), |_| true));
}
