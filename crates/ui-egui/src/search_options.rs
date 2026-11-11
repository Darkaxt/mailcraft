// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search box's inline options (Match case, Match whole word, Regex, and where to look) and the
//! relevance of free-text matches in the message list.
//!
//! Everything this UI reads from or sends to the engine about them is here, in one place: the
//! session's `view.search_options` (read through its JSON form), the `view.searchOptions` command,
//! `SortBy::Relevance`, and the `score` / `scoreDetail` a row of a free-text search carries.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::list::Row;
use mailcraft_engine::{Session, SortBy};
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::{Tokens, TypeRung, regular, semibold};

/// Where a free-text search looks, by the engine's code and by name.
pub(crate) const FIELDS: [(&str, &str); 4] = [("subject", "Subject"), ("people", "People"), ("body", "Body"), ("attachments", "Attachments")];

/// The search options in effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    pub match_case: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Codes from [`FIELDS`]; never empty (empty in the engine means all of them).
    pub search_in: Vec<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options { match_case: false, whole_word: false, regex: false, search_in: FIELDS.iter().map(|(code, _)| (*code).to_string()).collect() }
    }
}

impl Options {
    /// The options as the session holds them. Read through their JSON form, in either spelling of
    /// the field names, so that only this function knows how the engine stores them.
    pub(crate) fn of(s: &Session) -> Options {
        let v = serde_json::to_value(&s.view.search_options).unwrap_or(Value::Null);
        Options::from_json(&v)
    }

    pub(crate) fn from_json(v: &Value) -> Options {
        let flag = |camel: &str, snake: &str| v.get(camel).or_else(|| v.get(snake)).and_then(Value::as_bool).unwrap_or(false);
        let mut search_in: Vec<String> = v
            .get("searchIn")
            .or_else(|| v.get("search_in"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_ascii_lowercase)
            .filter(|code| FIELDS.iter().any(|(known, _)| known == code))
            .collect();
        if search_in.is_empty() {
            search_in = Options::default().search_in;
        }
        Options { match_case: flag("matchCase", "match_case"), whole_word: flag("wholeWord", "whole_word"), regex: flag("regex", "regex"), search_in }
    }

    /// The parameters of `view.searchOptions` that set these options.
    pub(crate) fn params(&self) -> Value {
        // Whole words mean nothing to a regular expression, which says its own boundaries.
        json!({"matchCase": self.match_case, "wholeWord": self.whole_word && !self.regex, "regex": self.regex, "searchIn": self.search_in})
    }
}

/// Sets the search options (a safe view command); a refusal is said as any other.
fn set(app: &mut MailApp, options: &Options) {
    if let Err(why) = app.execute("view.searchOptions", options.params()) {
        app.say_refused("view.searchOptions", &why);
    }
}

/// Whether the search in effect has free text (words to look for, not only filters such as
/// `from:`, `has:attachment` or dates): what relevance ranks and the Score column shows.
pub(crate) fn free_text_active(s: &Session) -> bool {
    let query = mailcraft_model::search::Query::parse(&s.view.search);
    (0..query.len()).any(|i| query.text_of(i).is_some() && !query.term_negated(i) && !query.is_address_term(i))
}

/// The arrangements the list's sort menu offers: the engine's, with By Relevance among them.
pub(crate) fn sort_choices() -> Vec<SortBy> {
    let mut all = SortBy::ALL.to_vec();
    if !all.contains(&SortBy::Relevance) {
        all.push(SortBy::Relevance);
    }
    all
}

/// The width the option toggles take at the right end of the search box.
pub(crate) const TOGGLES_W: f32 = 3.0 * TOGGLE + FIELDS_W + 4.0;
const TOGGLE: f32 = 26.0;
const FIELDS_W: f32 = 30.0;

/// The inline toggles at the right end of the search box, ending at `right`: Match case (Aa),
/// Match whole word, Regex (.*), and the "search in" dropdown. `free`: the window takes commands
/// (no dialog over it).
pub(crate) fn toggles(app: &mut MailApp, ui: &mut Ui, rect: Rect, right: f32, free: bool) {
    let t = Tokens::get(ui.ctx());
    let options = Options::of(&app.session);
    let mut x = right - TOGGLES_W;
    let items: [(&str, &str, &str, bool, bool); 3] = [
        ("searchCase", "Aa", tl!("Match case"), options.match_case, true),
        ("searchWholeWord", "ab", tl!("Match whole word"), options.whole_word && !options.regex, !options.regex),
        ("searchRegex", ".*", tl!("Use regular expression"), options.regex, true),
    ];
    for (key, glyph, name, on, enabled) in items {
        let area = Rect::from_center_size(pos2(x + TOGGLE / 2.0, rect.center().y), vec2(TOGGLE - 2.0, 22.0));
        x += TOGGLE;
        let response = ui.interact(area, ui.scope_id().with(("search_option", key)), if enabled { Sense::click() } else { Sense::hover() });
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, on, name));
        app.hit.add(key, 0, area);
        if on {
            ui.painter().rect(area, 4.0, t.checked, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        } else if enabled && response.hovered() {
            ui.painter().rect_filled(area, 4.0, t.hover);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(area.expand(1.0), 5.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Outside);
        }
        let ink = if !enabled {
            t.text_disabled
        } else if on {
            t.accent_text
        } else {
            t.text_dim
        };
        let font = if on { semibold(12.0) } else { regular(12.0) };
        ui.painter().text(area.center(), Align2::CENTER_CENTER, glyph, font, ink);
        // Match whole word: the letters stand on a bracket, as editors draw it.
        if key == "searchWholeWord" {
            let y = area.center().y + 6.5;
            let (l, r) = (area.center().x - 7.0, area.center().x + 7.0);
            ui.painter().line(vec![pos2(l, y - 2.5), pos2(l, y), pos2(r, y), pos2(r, y - 2.5)], Stroke::new(1.0, ink));
        }
        let tip = if enabled { name.to_string() } else { format!("{name} — {}", tl!("not with a regular expression")) };
        if response.on_hover_text(tip).clicked() && free && enabled {
            let mut next = options.clone();
            match key {
                "searchCase" => next.match_case = !on,
                "searchWholeWord" => next.whole_word = !on,
                _ => next.regex = !on,
            }
            set(app, &next);
        }
    }
    // Where to look: a small dropdown with a check for each part of a message.
    let area = Rect::from_center_size(pos2(x + FIELDS_W / 2.0, rect.center().y), vec2(FIELDS_W - 2.0, 22.0));
    let all = options.search_in.len() == FIELDS.len();
    let response = ui.interact(area, ui.scope_id().with("search_in"), Sense::click());
    let names: Vec<&str> = FIELDS.iter().filter(|(code, _)| options.search_in.iter().any(|c| c == code)).map(|(_, name)| tl!(name)).collect();
    let summary = if all { tl!("Everything").to_string() } else { names.join(", ") };
    let label = format!("{}: {summary}", tl!("Search in"));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &label));
    app.hit.add("searchIn", 0, area);
    if !all {
        ui.painter().rect(area, 4.0, t.checked, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    } else if response.hovered() {
        ui.painter().rect_filled(area, 4.0, t.hover);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(area.expand(1.0), 5.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Outside);
    }
    let ink = if all { t.text_dim } else { t.accent_text };
    crate::icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(area.center().x - 4.0, area.center().y), vec2(13.0, 13.0)),
        "filter",
        ink,
        t.accent,
    );
    let c = pos2(area.max.x - 6.0, area.center().y + 1.0);
    ui.painter().line(vec![c + vec2(-3.0, -1.5), c + vec2(0.0, 1.5), c + vec2(3.0, -1.5)], Stroke::new(1.1, ink));
    let response = response.on_hover_text(label.clone());
    egui::Popup::menu(&response).show(|ui| {
        ui.set_min_width(150.0);
        ui.label(egui::RichText::new(tl!("Search in")).font(TypeRung::Caption.regular()).weak());
        for (code, name) in FIELDS {
            let mut on = options.search_in.iter().any(|c| c == code);
            // The last part left stays: a search looks somewhere.
            let last = on && options.search_in.len() == 1;
            if ui.add_enabled(!last, egui::Checkbox::new(&mut on, tl!(name))).changed() && free {
                let mut next = options.clone();
                next.search_in.retain(|c| c != code);
                if on {
                    next.search_in.push(code.to_string());
                }
                // In the engine's order.
                next.search_in.sort_by_key(|c| FIELDS.iter().position(|(known, _)| known == c));
                set(app, &next);
            }
        }
    });
}

/// A row's relevance in a free-text search: its score (0 to 100) and, per term, how it matched
/// (`voice → invoice (+2 letters)`). `None` for a row without one.
pub(crate) fn score(row: &Row) -> Option<(u8, Vec<String>)> {
    let v = serde_json::to_value(row).ok()?;
    let score = v.get("score").and_then(Value::as_u64)?.min(100) as u8;
    let detail = v.get("scoreDetail").or_else(|| v.get("score_detail")).and_then(Value::as_array).cloned().unwrap_or_default();
    Some((score, detail.iter().map(detail_line).collect()))
}

/// One term's line of a score's explanation: what was typed, what it matched, how far apart they
/// are, and where.
pub(crate) fn detail_line(d: &Value) -> String {
    let term = d.get("term").and_then(Value::as_str).unwrap_or("");
    let matched = d.get("matched").and_then(Value::as_str).unwrap_or(term);
    let extra = d.get("extra").and_then(Value::as_i64).unwrap_or(0);
    let mut line = if matched.is_empty() || matched == term { term.to_string() } else { format!("{term} → {matched}") };
    if extra != 0 {
        let n = extra.unsigned_abs();
        let letters = if n == 1 { tl!("letter") } else { tl!("letters") };
        line.push_str(&format!(" ({}{n} {letters})", if extra > 0 { "+" } else { "−" }));
    }
    if let Some(field) = d.get("field").and_then(Value::as_str).filter(|f| !f.is_empty()) {
        line.push_str(&format!(" · {field}"));
    }
    line
}

#[cfg(test)]
#[path = "search_options_tests.rs"]
mod tests;
