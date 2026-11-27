// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you as a list of its own above the message list (v0.12.45): its rows with a scroll
//! of their own (its title is the list header's, v0.12.46), the divider under it that the owner
//! drags, and the Mails heading of the list below, with All and Unread. Both lists are in view at
//! once; the list below still lists every message, For you's among them.

use egui::{CursorIcon, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::list::Row;
use mailcraft_model::Id;

use super::{Look, SENDER_TEXT, TEXT_LEFT, filter_tabs, lane_width, message_row, overlay_scroll_style, scroll_style};
use crate::MailApp;
use crate::theme::{Tokens, paint_clipped, regular};

/// The divider under For you, where the owner drags it.
pub(super) const DIVIDER_H: f32 = 7.0;
/// How much of the list's height For you takes until the owner drags its divider.
const DEFAULT_SHARE: f32 = 0.4;
/// The rows of the list below that always stay in view.
const REST_ROWS: f32 = 3.0;
/// The Mails heading over the list below.
pub(super) const MAILS_H: f32 = 36.0;
/// All and Unread on the Mails heading.
const TAB_TEXT: f32 = 13.0;
/// The line under For you's rows with Show all, when the header has no room for it.
const SHOW_ALL_H: f32 = 30.0;

/// How tall For you is in `room` with `rows` rows of `row_h`, `wanted` tall (0: its share of
/// `room`): at least a row, never so tall that the list below has fewer than
/// [`REST_ROWS`] rows, and no taller than what it lists (and `extra` under its rows). Also the
/// least and the most it may be dragged to.
pub(super) fn height(room: f32, rows: usize, row_h: f32, extra: f32, wanted: f32) -> (f32, f32, f32) {
    let content = rows as f32 * row_h + extra;
    let least = row_h.min(content);
    let most = (room - DIVIDER_H - REST_ROWS * row_h).max(least);
    let wanted = if wanted > 0.0 { wanted } else { room * DEFAULT_SHARE };
    (wanted.clamp(least, most).min(content), least, most)
}

/// Draws For you's `rows` at the top of `ui`, the divider and the Mails heading, and moves its
/// cursor under them.
/// `reveal`: the row to bring into view; `show_all_row`: Show all ends the list (the header had no
/// room for it). The rows drawn, for the swipe that may be on one.
#[allow(clippy::too_many_arguments)]
pub(super) fn show(
    app: &mut MailApp,
    ui: &mut Ui,
    rows: &[Row],
    row_h: f32,
    look: Look<'_>,
    scope: crate::swipe::Scope,
    reveal: Option<(usize, bool)>,
    show_all_row: bool,
) -> Vec<Id> {
    let t = Tokens::get(ui.ctx());
    let outer = ui.available_rect_before_wrap();
    let extra = if show_all_row { SHOW_ALL_H } else { 0.0 };
    let (h, least, most) = height(outer.height() - MAILS_H, rows.len(), row_h, extra, app.ui.for_you_height);
    let panel = Rect::from_min_size(outer.min, vec2(outer.width(), h));
    let lane_w = lane_width(app);
    let content = rows.len() as f32 * row_h + extra;
    let overlay = app.overlay_scroll_bars;
    let mut seen = Vec::new();
    ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
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
            if let Some(label) = crate::for_you_view::show_all_label(app, rows.first().is_some_and(|r| r.for_you_more)).filter(|_| show_all_row) {
                let y = area.min.y + content - SHOW_ALL_H / 2.0;
                crate::for_you_view::show_all(app, ui, pos2(area.min.x + TEXT_LEFT + 1.0, y), label);
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
    // The list below's heading, in For you's title's font, with All and Unread at its right:
    // they filter that list.
    let mails = Rect::from_min_size(pos2(outer.min.x, divider.max.y), vec2(outer.width(), MAILS_H));
    let end = mails.max.x - lane_w - 8.0;
    let left = mails.min.x + TEXT_LEFT + 1.0;
    let w = paint_clipped(ui.painter(), pos2(left, mails.center().y), (end - left).max(0.0), tl!("Mails"), regular(SENDER_TEXT), t.text);
    app.hit.add("mailsHeading", 0, mails);
    let band = Rect::from_min_max(pos2(mails.min.x, mails.min.y + 4.0), mails.max);
    filter_tabs(app, ui, band, mails.center().y, left + w + 24.0, end, TAB_TEXT, true);
    ui.advance_cursor_after_rect(Rect::from_min_max(outer.min, mails.max));
    seen
}
