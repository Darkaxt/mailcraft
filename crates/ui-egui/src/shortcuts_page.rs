// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › Keyboard shortcuts (v0.12.12): the keys of each command, which the owner
//! changes by pressing new ones. A key alone only moves around (arrows, Page Up, Home…);
//! whatever changes mail takes two keys or more ([`crate::keys::check_keys`]).

use egui::{Key, Sense, Ui, vec2};
use mailcraft_engine::CommandSpec;
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::{Tokens, regular};

/// Most rows a search shows at once.
const MOST: usize = 60;

#[derive(Clone, Default)]
struct State {
    /// What the search box says.
    find: String,
    /// The command whose new keys are being pressed now.
    capturing: Option<&'static str>,
    /// Why the last keys pressed were refused, and for which command.
    refused: Option<(&'static str, String)>,
}

fn store() -> egui::Id {
    egui::Id::unique("options_shortcuts")
}

/// Whether the list shows `c`: with a search, what it matches; without, the commands with keys.
fn shown(c: &CommandSpec, find: &str) -> bool {
    if find.is_empty() {
        return !c.keys().is_empty() || !c.shortcut.is_empty();
    }
    if c.reads && c.shortcut.is_empty() {
        return false;
    }
    let find = find.to_lowercase();
    tl!(c.label).to_lowercase().contains(&find) || c.label.to_lowercase().contains(&find) || c.keys().to_lowercase().contains(&find)
}

/// Keys as the page shows them: `Mod+Shift+Delete` is Ctrl+Shift+Delete, each key's name in the
/// interface language (Mayús in Spanish).
fn shown_keys(keys: &str) -> String {
    crate::widgets::key_text(keys).split('+').map(key_word).collect::<Vec<_>>().join("+")
}

/// A key's name in the interface language; a letter, a digit, Ctrl or F1… as it is.
fn key_word(word: &str) -> &str {
    match word {
        "Shift" => tl!("Shift"),
        "Alt" => tl!("Alt"),
        "Delete" => tl!("Delete"),
        "Up" => tl!("Up"),
        "Down" => tl!("Down"),
        "Left" => tl!("Left"),
        "Right" => tl!("Right"),
        "Home" => tl!("Home"),
        "End" => tl!("End"),
        "PageUp" => tl!("PageUp"),
        "PageDown" => tl!("PageDown"),
        "Escape" => tl!("Escape"),
        "Enter" => tl!("Enter"),
        "Plus" => tl!("Plus"),
        "Minus" => tl!("Minus"),
        "Space" => tl!("Space"),
        "Tab" => tl!("Tab"),
        "Backspace" => tl!("Backspace"),
        "Insert" => tl!("Insert"),
        other => other,
    }
}

/// The keys pressed this frame, as the registry writes them (`Mod+Shift+D`): the first press that
/// is a key with a name. Escape alone is left to cancel.
fn pressed(ui: &Ui) -> Option<String> {
    ui.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } if !(*key == Key::Escape && modifiers.is_none()) => {
                crate::keys::key_name(*key).map(|name| crate::keys::combo(*modifiers, name))
            }
            _ => None,
        })
    })
}

pub(crate) fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let mut st: State = ui.ctx().data(|d| d.get_temp(store())).unwrap_or_default();
    ui.label(
        egui::RichText::new(tl!("A key alone only moves around. Anything that changes mail needs two keys, such as Ctrl+D."))
            .font(regular(12.0))
            .color(t.text_dim),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let find = egui::TextEdit::singleline(&mut st.find).hint_text(tl!("Find a command")).desired_width(260.0);
        let find = crate::text_menu::field(ui.add(find));
        app.hit.add("shortcutsFind", 0, find.rect);
        if !app.ui.shortcuts.is_empty() {
            let reset = ui.button(tl!("Reset all"));
            app.hit.add("shortcutsResetAll", 0, reset.rect);
            if reset.on_hover_text(tl!("Every command gets its original keys back")).clicked() {
                let _ = app.run("ui.shortcut", json!({"keys": Value::Null}));
                st.capturing = None;
                st.refused = None;
            }
        }
    });
    ui.add_space(4.0);
    // While new keys are being pressed, the first press is them.
    if let Some(id) = st.capturing
        && let Some(keys) = pressed(ui)
    {
        st.capturing = None;
        match app.execute("ui.shortcut", json!({"command": id, "keys": keys})) {
            Ok(_) => st.refused = None,
            Err(e) => st.refused = Some((id, tl!(&e).to_string())),
        }
    }
    let registry = app.session.registry.clone();
    let find = st.find.trim().to_string();
    let rows: Vec<&CommandSpec> = registry.all().iter().filter(|c| shown(c, &find)).take(MOST).collect();
    if rows.is_empty() {
        ui.label(egui::RichText::new(tl!("No command matches")).color(t.text_dim));
    }
    egui::Grid::new("options_shortcuts_grid").num_columns(3).spacing(vec2(12.0, 4.0)).striped(true).show(ui, |ui| {
        for c in rows {
            let label = ui.label(egui::RichText::new(tl!(c.label)).color(t.text));
            label.on_hover_text(tl!(c.location));
            let keys = c.keys();
            let capturing = st.capturing == Some(c.id);
            let text = if capturing {
                tl!("Press the new keys…").to_string()
            } else if keys.is_empty() {
                tl!("None").to_string()
            } else {
                keys.split(" / ").map(shown_keys).collect::<Vec<_>>().join(", ")
            };
            let (r, resp) = ui.allocate_exact_size(vec2(200.0, 24.0), Sense::click());
            let fill = if capturing {
                t.checked
            } else if resp.hovered() {
                t.hover
            } else {
                t.panel
            };
            ui.painter().rect(r, 3.0, fill, egui::Stroke::new(1.0, if capturing { t.accent } else { t.border }), egui::StrokeKind::Inside);
            let color = if keys.is_empty() && !capturing { t.text_dim } else { t.text };
            ui.painter().text(r.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, text, regular(12.5), color);
            app.hit.add("shortcutKeys", 0, r);
            if resp.on_hover_text(tl!("Click, then press the new keys")).clicked() {
                st.capturing = if capturing { None } else { Some(c.id) };
                st.refused = None;
            }
            ui.horizontal(|ui| {
                if capturing {
                    if ui.button(tl!("Cancel")).clicked() {
                        st.capturing = None;
                    }
                    return;
                }
                if !keys.is_empty() {
                    let clear = ui.small_button("✕").on_hover_text(tl!("No keys"));
                    if clear.clicked() {
                        let _ = app.execute("ui.shortcut", json!({"command": c.id, "keys": ""}));
                    }
                }
                if app.ui.shortcuts.contains_key(c.id) && ui.small_button(tl!("Reset")).on_hover_text(tl!("Back to its original keys")).clicked() {
                    let _ = app.execute("ui.shortcut", json!({"command": c.id, "keys": Value::Null}));
                }
                if let Some((id, why)) = &st.refused
                    && *id == c.id
                {
                    ui.label(egui::RichText::new(why).color(t.red));
                }
            });
            ui.end_row();
        }
    });
    if st.capturing.is_some() {
        ui.ctx().request_repaint();
    }
    ui.ctx().data_mut(|d| d.insert_temp(store(), st));
}
