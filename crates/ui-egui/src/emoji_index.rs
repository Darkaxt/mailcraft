// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The emoji a folder's Tag menu offers (#fork, v0.12.22): a few suggested ones, then every emoji
//! Unicode names, by its groups, with a search over their names and short codes. Left out: what the
//! window's fonts can't draw, and on Windows the flags of countries and regions, which its emoji
//! face draws as two letters.

use std::sync::Arc;

use egui::{Context, FontFamily, Id};
use emojis::{Emoji, Group};

/// The emoji first shown, and what tags there were before the index.
pub(crate) const SUGGESTED: &[&str] = &[
    "⭐", "📌", "🔥", "✅", "💼", "🏦", "💳", "📄", "💰", "📦", "✈", "🚆", "🏠", "🎮", "🎬", "🎵", "📚", "🎓", "💻", "🔒", "☁", "📰", "📣", "🏥",
    "⚽", "🍔", "🎁", "👪", "⚖", "📁",
];

/// The menu's tabs: the suggested emoji, then Unicode's groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum Tab {
    #[default]
    Suggested,
    Group(Group),
}

/// Each tab, the emoji on its button and its name.
pub(crate) const TABS: &[(Tab, &str, &str)] = &[
    (Tab::Suggested, "⭐", tl_later!("Suggested")),
    (Tab::Group(Group::SmileysAndEmotion), "😀", tl_later!("Smileys & emotion")),
    (Tab::Group(Group::PeopleAndBody), "👋", tl_later!("People & body")),
    (Tab::Group(Group::AnimalsAndNature), "🐻", tl_later!("Animals & nature")),
    (Tab::Group(Group::FoodAndDrink), "🍔", tl_later!("Food & drink")),
    (Tab::Group(Group::TravelAndPlaces), "✈", tl_later!("Travel & places")),
    (Tab::Group(Group::Activities), "⚽", tl_later!("Activities")),
    (Tab::Group(Group::Objects), "💡", tl_later!("Objects")),
    (Tab::Group(Group::Symbols), "💯", tl_later!("Symbols")),
    (Tab::Group(Group::Flags), "🏁", tl_later!("Flags")),
];

/// How many characters a tag may have (`folder.tag`).
const MAX_TAG: usize = 16;

/// A country's or a region's flag: regional indicator letters, or a subdivision's tag sequence.
fn place_flag(s: &str) -> bool {
    s.chars().any(|c| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c) || ('\u{E0020}'..='\u{E007F}').contains(&c))
}

/// Whether `e` is offered, given the characters the fonts have (`has`): each one it is made of,
/// save the joiner and the selector that asks for the picture, and not a place's flag on Windows.
pub(crate) fn offered(e: &Emoji, has: impl Fn(char) -> bool) -> bool {
    let s = e.as_str();
    s.chars().count() <= MAX_TAG && !(cfg!(windows) && place_flag(s)) && s.chars().filter(|c| !matches!(c, '\u{200D}' | '\u{FE0F}')).all(has)
}

/// The emoji offered, in Unicode's order; worked out again only when the window's fonts change.
pub(crate) fn listed(ctx: &Context) -> Arc<Vec<&'static Emoji>> {
    let key = Id::unique("emoji_index_listed");
    let fonts = ctx.fonts(|f| f.definitions().font_data.len());
    if let Some((n, list)) = ctx.data(|d| d.get_temp::<(usize, Arc<Vec<&'static Emoji>>)>(key))
        && n == fonts
    {
        return list;
    }
    let list = Arc::new(ctx.fonts_mut(|f| {
        let chars = f.characters(&FontFamily::Proportional);
        emojis::iter().filter(|e| offered(e, |c| chars.contains_key(&c))).collect::<Vec<_>>()
    }));
    ctx.data_mut(|d| d.insert_temp(key, (fonts, list.clone())));
    list
}

/// What a tab shows, each emoji with its name.
pub(crate) fn in_tab(list: &[&'static Emoji], tab: Tab) -> Vec<(&'static str, &'static str)> {
    match tab {
        Tab::Suggested => SUGGESTED.iter().map(|s| (*s, emojis::get(s).map_or("", Emoji::name))).collect(),
        Tab::Group(g) => list.iter().filter(|e| e.group() == g).map(|e| (e.as_str(), e.name())).collect(),
    }
}

/// The emoji whose name or a short code has every word of `query` in it, in Unicode's order; an
/// emoji typed or pasted in finds itself.
pub(crate) fn search(list: &[&'static Emoji], query: &str) -> Vec<(&'static str, &'static str)> {
    let query = query.trim();
    if let Some(e) = emojis::get(query) {
        return vec![(e.as_str(), e.name())];
    }
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    if words.is_empty() {
        return Vec::new();
    }
    list.iter()
        .filter(|e| {
            let name = e.name().to_lowercase();
            words.iter().all(|w| name.contains(w.as_str()) || e.shortcodes().any(|code| code.contains(w.as_str())))
        })
        .map(|e| (e.as_str(), e.name()))
        .collect()
}

#[cfg(test)]
#[path = "emoji_index_tests.rs"]
mod emoji_index_tests;
