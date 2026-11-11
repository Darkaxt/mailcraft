// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Combined inbox navigation and theme-adapted, locally persisted account colours.

use crate::{
    MailApp,
    theme::{Tokens, semibold},
};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Scope;
use mailcraft_model::{FolderKind, Id, Rgb};
use serde_json::{Value, json};

pub(crate) fn navigation(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let selected = app.session.view.scope == Scope::Inboxes && app.session.view.search_folder.is_none();
    let counts = app.session.folder_counts();
    let unread =
        app.session.mailbox.folders.iter().filter(|f| f.kind == FolderKind::Inbox).fold(0usize, |total, f| total.saturating_add(counts.unread(f.id)));
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let card = rect.shrink2(vec2(6.0, 1.0));
    if selected || response.hovered() {
        ui.painter().rect_filled(card, 5.0, if selected { t.selection } else { t.hover });
    }
    if response.has_focus() {
        ui.painter().rect_stroke(card, 5.0, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
    }
    crate::icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(16.0, 16.0)), "mail", t.icon, t.accent);
    crate::theme::paint_clipped(
        ui.painter(),
        pos2(rect.left() + 38.0, rect.center().y),
        (rect.width() - 86.0).max(0.0),
        tl!("All inboxes"),
        semibold(12.0),
        t.text,
    );
    if unread > 0 {
        ui.painter().text(pos2(rect.right() - 16.0, rect.center().y), Align2::RIGHT_CENTER, unread.to_string(), semibold(11.0), t.text);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("All inboxes")));
    app.hit.add("allInboxes", 0, rect);
    if response.clicked() {
        let _ = app.run("view.allInboxes", json!({}));
    }
    ui.add_space(6.0);
}

fn luminance(color: Color32) -> f32 {
    let linear = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub(crate) fn mix(a: Color32, b: Color32, weight: f32) -> Color32 {
    let channel = |a: u8, b: u8| (f32::from(a) * (1.0 - weight) + f32::from(b) * weight).round() as u8;
    Color32::from_rgb(channel(a.r(), b.r()), channel(a.g(), b.g()), channel(a.b(), b.b()))
}

/// Hue comes from the account; text and high-contrast states retain their readable surface.
pub(crate) fn row_tint(t: &Tokens, color: Rgb) -> Option<Color32> {
    if t.high_contrast {
        return None;
    }
    let pastel = if color.0.min(color.1).min(color.2) < 170 {
        let c = |v: u8| 170 + ((u16::from(v) * 85) / 255) as u8;
        Color32::from_rgb(c(color.0), c(color.1), c(color.2))
    } else {
        crate::theme::c32(color)
    };
    let strength = if t.dark { 0.14 } else { 0.70 };
    for step in (0..=10).rev() {
        let result = mix(t.surface, pastel, strength * step as f32 / 10.0);
        if [t.text, t.text_dim, t.overdue].iter().all(|ink| contrast(*ink, result) >= 4.5) {
            return Some(result);
        }
    }
    None
}

pub(crate) fn color_menu(app: &mut MailApp, ui: &mut Ui, id: Id) {
    color_menu_for(app, ui, id, None);
}

pub(crate) fn color_menu_for(app: &mut MailApp, ui: &mut Ui, id: Id, alias: Option<&str>) {
    let Some(account) = app.session.mailbox.account(id) else { return };
    let selected = account.origin_row_color(alias);
    let custom = alias.map_or(account.row_color.is_some(), |alias| account.alias_row_colors.contains_key(&alias.to_ascii_lowercase()));
    let command = if alias.is_some() { "aliases.rowColor" } else { "account.rowColor" };
    let params = |color: Value| match alias {
        Some(alias) => json!({"account":id,"alias":alias,"color":color}),
        None => json!({"id":id,"color":color}),
    };
    ui.menu_button(tl!("Origin row color"), |ui| {
        let t = Tokens::get(ui.ctx());
        let scope = ui.scope_id();
        ui.label(tl!("Origin color in combined views"));
        ui.horizontal(|ui| {
            for color in mailcraft_model::ACCOUNT_ROW_COLORS {
                let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                ui.painter().rect(
                    rect.shrink(2.0),
                    4.0,
                    crate::theme::c32(color),
                    Stroke::new(if color == selected { 2.0 } else { 1.0 }, t.border_strong),
                    egui::StrokeKind::Inside,
                );
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, color.hex()));
                if response.on_hover_text(color.hex()).clicked() {
                    let _ = app.run(command, params(json!(color.hex())));
                    ui.ctx().data_mut(|data| data.remove::<String>(custom_color_edit_id(scope, id, alias, selected)));
                    ui.close();
                }
            }
        });
        // Keyed by the colour now in effect: a swatch, Reset or another writer changing it
        // never brings back text typed against the previous colour.
        let edit_id = custom_color_edit_id(scope, id, alias, selected);
        let mut text = ui.ctx().data_mut(|data| data.get_temp::<String>(edit_id)).unwrap_or_else(|| selected.hex());
        let mut done = false;
        ui.horizontal(|ui| {
            ui.label(tl!("Custom color"));
            ui.add(egui::TextEdit::singleline(&mut text).char_limit(7).desired_width(76.0));
            let parsed = Rgb::parse(&text);
            if ui.add_enabled(parsed.is_some(), egui::Button::new(tl!("Apply"))).clicked() {
                if let Some(color) = parsed {
                    let _ = app.run(command, params(json!(color.hex())));
                }
                done = true;
                ui.close();
            }
        });
        if ui.add_enabled(custom, egui::Button::new(tl!("Reset color"))).clicked() {
            let _ = app.run(command, params(Value::Null));
            done = true;
            ui.close();
        }
        if done {
            ui.ctx().data_mut(|data| data.remove::<String>(edit_id));
        } else {
            ui.ctx().data_mut(|data| data.insert_temp(edit_id, text));
        }
    });
}

/// Where the custom-colour field keeps its text between frames, for the colour in effect.
pub(crate) fn custom_color_edit_id(scope: egui::Id, account: Id, alias: Option<&str>, selected: Rgb) -> egui::Id {
    scope.with(("account_row_color", account, alias, selected))
}

#[cfg(test)]
#[path = "unified_inbox_tests.rs"]
mod tests;
