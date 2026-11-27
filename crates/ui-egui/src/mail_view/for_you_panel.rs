// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you as a list of its own above the message list (v0.12.45): its heading, its rows
//! with a scroll of their own, and the divider under it that the owner drags. Both lists are in
//! view at once; the list below still lists every message, For you's among them.

use egui::{CursorIcon, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::list::Row;
use mailcraft_model::Id;

use super::{GROUP_H, GROUP_TEXT, Look, ROW_INSET, lane_width, message_row, overlay_scroll_style, scroll_style};
use crate::MailApp;
use crate::theme::Tokens;

/// The divider under For you, where the owner drags it.
pub(super) const DIVIDER_H: f32 = 7.0;
/// How much of the list's height For you takes until the owner drags its divider.
const DEFAULT_SHARE: f32 = 0.4;
/// The rows of the list below that always stay in view.
const REST_ROWS: f32 = 3.0;

/// How tall For you is in `room` with `rows` rows of `row_h`, `wanted` tall (0: its share of
/// `room`): at least its heading and a row, never so tall that the list below has fewer than
/// [`REST_ROWS`] rows, and no taller than what it lists. Also the least and the most it may be
/// dragged to.
pub(super) fn height(room: f32, rows: usize, row_h: f32, wanted: f32) -> (f32, f32, f32) {
    let content = rows as f32 * row_h;
    let least = GROUP_H + row_h.min(content);
    let most = (room - DIVIDER_H - REST_ROWS * row_h).max(least);
    let wanted = if wanted > 0.0 { wanted } else { room * DEFAULT_SHARE };
    (wanted.clamp(least, most).min(GROUP_H + content), least, most)
}

/// Draws For you's `rows` at the top of `ui` and moves its cursor under them and the divider.
/// `reveal`: the row to bring into view. The rows drawn, for the swipe that may be on one.
pub(super) fn show(
    app: &mut MailApp,
    ui: &mut Ui,
    rows: &[Row],
    row_h: f32,
    look: Look<'_>,
    scope: crate::swipe::Scope,
    reveal: Option<(usize, bool)>,
) -> Vec<Id> {
    let t = Tokens::get(ui.ctx());
    let outer = ui.available_rect_before_wrap();
    let (h, least, most) = height(outer.height(), rows.len(), row_h, app.ui.for_you_height);
    let panel = Rect::from_min_size(outer.min, vec2(outer.width(), h));
    let lane_w = lane_width(app);
    let heading = Rect::from_min_max(pos2(panel.min.x + ROW_INSET, panel.min.y), pos2(panel.max.x - lane_w, panel.min.y + GROUP_H));
    crate::for_you_view::header(app, ui, heading, rows.len(), rows.first().is_some_and(|r| r.for_you_more), GROUP_TEXT);
    let body = Rect::from_min_max(pos2(panel.min.x, panel.min.y + GROUP_H), panel.max);
    let content = rows.len() as f32 * row_h;
    let overlay = app.overlay_scroll_bars;
    let mut seen = Vec::new();
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        let plain = ui.style().clone();
        if overlay {
            overlay_scroll_style(ui, &t);
        } else {
            scroll_style(ui, &t);
        }
        egui::ScrollArea::vertical().id_salt("for_you_list").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
            ui.set_style(plain);
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), content), Sense::hover());
            let width = (area.width() - lane_w).max(0.0);
            let rect_of = |i: usize| Rect::from_min_size(pos2(area.min.x, area.min.y + i as f32 * row_h), vec2(width, row_h));
            if let Some((i, centered)) = reveal {
                ui.scroll_to_rect(rect_of(i), centered.then_some(egui::Align::Center));
            }
            let first = (viewport.min.y / row_h).floor().max(0.0) as usize;
            for (i, row) in rows.iter().enumerate().skip(first) {
                if i as f32 * row_h > viewport.max.y {
                    break;
                }
                seen.push(row.id);
                message_row(app, ui, row, rect_of(i), look, scope);
            }
        });
    });
    // The divider: a line, stronger under the pointer and while dragged.
    let divider = Rect::from_min_size(pos2(outer.min.x, panel.max.y), vec2(outer.width(), DIVIDER_H));
    let resp = ui
        .interact(divider, ui.scope_id().with("for_you_divider"), Sense::drag())
        .on_hover_cursor(CursorIcon::ResizeVertical)
        .on_hover_text(tl!("Drag to make For you taller or shorter"));
    app.hit.add("forYouDivider", 0, divider);
    let active = resp.hovered() || resp.dragged();
    let stroke = if active { Stroke::new(2.0, t.accent) } else { Stroke::new(1.0, t.border_strong) };
    ui.painter().hline(divider.x_range(), divider.center().y, stroke);
    if resp.dragged() {
        app.ui.for_you_height = (h + resp.drag_delta().y).clamp(least, most);
    }
    ui.advance_cursor_after_rect(Rect::from_min_max(outer.min, divider.max));
    seen
}
