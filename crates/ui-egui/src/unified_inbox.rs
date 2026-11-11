// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The unified mailbox root's helpers, origin colours (a stripe or the full row) and their menus.

use crate::{
    MailApp,
    theme::{Tokens, regular},
};
use egui::{Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{AccountKind, Id, Rgb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The id the "Unified mailbox" root is folded under in `UiState::collapsed_folders`, beside the
/// Favorites' (0) and the accounts' and folders' ids, which are numbered from 1 and never reach it
/// (kept within what a JSON number holds exactly, for agents reading the settings).
pub(crate) const UNIFIED_ROOT: Id = (1 << 53) - 1;

/// How rows of combined views (unified inbox, filing tree, search folders) show where they came
/// from: a slim bar of the origin colour at the row's left edge, or that colour over the whole row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OriginColors {
    #[default]
    Stripe,
    FullRow,
}

impl OriginColors {
    pub const ALL: [OriginColors; 2] = [OriginColors::Stripe, OriginColors::FullRow];

    pub(crate) fn label(self) -> &'static str {
        match self {
            OriginColors::Stripe => tl!("Stripe"),
            OriginColors::FullRow => tl!("Full row"),
        }
    }
}

/// The accounts whose real folder tree can be the unified filing tree: the owner's own Microsoft
/// accounts, in the pane's order, each with its address.
pub(crate) fn filing_accounts(app: &MailApp) -> Vec<(Id, String)> {
    app.session
        .mailbox
        .accounts_in_order()
        .into_iter()
        .filter(|a| a.kind == AccountKind::Microsoft && !a.is_shared())
        .map(|a| (a.id, if a.email.is_empty() { a.name.clone() } else { a.email.clone() }))
        .collect()
}

/// The filing account whose tree the unified mailbox shows: the one in view, else the one the
/// owner picked last (while it still qualifies), else the first.
pub(crate) fn filing_account(app: &MailApp, accounts: &[(Id, String)]) -> Option<Id> {
    let qualifies = |id: &Id| accounts.iter().any(|(a, _)| a == id);
    app.session.view.filing.filter(qualifies).or(app.ui.unified_filing_account.filter(qualifies)).or_else(|| accounts.first().map(|(id, _)| *id))
}

/// Whether the list in view is a combined one, whose rows wear their origin's colour.
pub(crate) fn combined_view(s: &mailcraft_engine::Session) -> bool {
    s.view.search_folder.is_some() || s.view.filing.is_some() || s.view.scope == mailcraft_engine::Scope::Inboxes
}

/// The root's context menu: the filing account (when there are several), a new search folder,
/// and how origin colours are drawn.
pub(crate) fn root_menu(app: &mut MailApp, ui: &mut Ui, accounts: &[(Id, String)], current: Option<Id>) {
    ui.set_min_width(220.0);
    if accounts.len() > 1 {
        ui.label(egui::RichText::new(tl!("Filing account")).font(regular(11.0)).weak());
        for (id, email) in accounts {
            if ui.selectable_label(current == Some(*id), email).clicked() {
                pick_filing_account(app, *id);
                ui.close();
            }
        }
        ui.separator();
    }
    if ui.button(tl!("New search folder…")).clicked() {
        let _ = app.run("ui.dialog", json!({"name":"searchFolder"}));
        ui.close();
    }
    ui.separator();
    ui.label(egui::RichText::new(tl!("Origin colors")).font(regular(11.0)).weak());
    for mode in OriginColors::ALL {
        if ui.selectable_label(app.ui.origin_colors == mode, mode.label()).clicked() {
            app.ui.origin_colors = mode;
            ui.close();
        }
    }
}

/// Remembers `id` as the unified filing account, and shows its tree at once while a filing view
/// is in front.
pub(crate) fn pick_filing_account(app: &mut MailApp, id: Id) {
    app.ui.unified_filing_account = Some(id);
    if app.session.view.filing.is_some_and(|now| now != id) {
        let _ = app.run("view.filing", json!({"account":id}));
    }
}

/// The small dropdown beside the list's sort menu that chooses how origin colours are drawn, in
/// `r`: a sample of the mode and a chevron.
pub(crate) fn mode_dropdown(app: &mut MailApp, ui: &mut Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    let mode = app.ui.origin_colors;
    let resp = ui.interact(r, ui.scope_id().with("origin_colors"), Sense::click());
    app.hit.add("originColors", 0, r);
    let name = format!("{}: {}", tl!("Origin colors"), mode.label());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, &name));
    if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, t.hover);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(r, 4.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
    }
    // The mode as a tiny row: a box with a bar down its left edge, or a tinted fill.
    let sample = Rect::from_center_size(pos2(r.min.x + 11.0, r.center().y), vec2(12.0, 10.0));
    let ink = crate::theme::c32(mailcraft_model::ACCOUNT_ROW_COLORS[0]);
    match mode {
        OriginColors::Stripe => {
            ui.painter().rect_stroke(sample, 2.0, Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
            ui.painter().rect_filled(Rect::from_min_size(sample.min, vec2(3.0, sample.height())), 1.0, ink);
        }
        OriginColors::FullRow => {
            ui.painter().rect_filled(sample, 2.0, mix(t.surface, ink, 0.45));
            ui.painter().rect_stroke(sample, 2.0, Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
        }
    }
    let c = pos2(r.max.x - 7.0, r.center().y + 1.0);
    ui.painter().line(vec![c + vec2(-3.5, -2.0), c + vec2(0.0, 2.0), c + vec2(3.5, -2.0)], Stroke::new(1.2, t.text));
    let resp = resp.on_hover_text(tl!("How rows of combined views show their account or alias color"));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(150.0);
        ui.label(egui::RichText::new(tl!("Origin colors")).font(regular(11.0)).weak());
        for option in OriginColors::ALL {
            if ui.selectable_label(app.ui.origin_colors == option, option.label()).clicked() {
                app.ui.origin_colors = option;
                ui.close();
            }
        }
    });
}

/// A small swatch of an origin colour (beside an account's root, an alias row), ringed so that a
/// colour close to the pane's still shows.
pub(crate) fn swatch(painter: &egui::Painter, t: &Tokens, center: egui::Pos2, color: Rgb) -> Rect {
    let r = Rect::from_center_size(center, vec2(10.0, 10.0));
    painter.rect_filled(r, 2.5, crate::theme::c32(color));
    let ring = if t.high_contrast { t.text } else { t.border_strong };
    painter.rect_stroke(r, 2.5, Stroke::new(1.0, ring), egui::StrokeKind::Inside);
    r
}

fn luminance(color: Color32) -> f32 {
    let linear = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
}

pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub(crate) fn mix(a: Color32, b: Color32, weight: f32) -> Color32 {
    let channel = |a: u8, b: u8| (f32::from(a) * (1.0 - weight) + f32::from(b) * weight).round() as u8;
    Color32::from_rgb(channel(a.r(), b.r()), channel(a.g(), b.g()), channel(a.b(), b.b()))
}

/// The full-row tint of an origin colour: the real colour mixed into the list's surface as
/// strongly as text, dimmed text and overdue text keep a contrast of 4.5 on it. `None` in High
/// Contrast (the stripe shows the origin there) or when no visible strength keeps that contrast.
pub(crate) fn row_tint(t: &Tokens, color: Rgb) -> Option<Color32> {
    if t.high_contrast {
        return None;
    }
    let color = crate::theme::c32(color);
    let strength = if t.dark { 0.32 } else { 0.24 };
    for step in (1..=10).rev() {
        let result = mix(t.surface, color, strength * step as f32 / 10.0);
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
