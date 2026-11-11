// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reader's floating controls: a small bar that stands over the top-right corner of the
//! message, clear of its scroll bar, instead of a row of its own above it. From left to right:
//! the tracker shield (only when the message has trackers), the source toggle (the reading pane
//! only), and the message appearance menu.
//!
//! It is painted in the window's own colours (the menu surface, a border and a soft shadow),
//! never in the message's, so it reads the same over a white newsletter, a dark design or the
//! source. Every control is a real button: focusable, pressed with Space or Enter, named for
//! screen readers, with a tooltip that says what it does.

use egui::{Align2, Color32, Layout, Rect, Response, Sense, Shape, Stroke, Ui, UiBuilder, pos2, vec2};
use mailcraft_model::Message;
use serde_json::json;

use super::{Place, appearance, view};
use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// Height of a control, and of the bar less its padding.
const HEIGHT: f32 = 26.0;
/// Width of an icon-only control.
const BUTTON: f32 = 30.0;
/// Padding inside the bar.
const PAD: f32 = 3.0;
/// Gap between the bar and the top of the message, and between the bar and the scroll bar.
const INSET: f32 = 8.0;

/// What the shield is told about a message's trackers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Trackers {
    pub count: usize,
    /// The owner said "Load anyway".
    pub kept: bool,
}

/// The bar over the top-right corner of `room` (where the body or the source is shown). `m` is
/// the message read; `source` offers the source toggle; `trackers` shows the shield.
pub(super) fn overlay(app: &mut MailApp, ui: &mut Ui, room: Rect, m: &Message, place: Place, source: bool, trackers: Option<Trackers>) {
    let t = Tokens::get(ui.ctx());
    let trackers = trackers.filter(|tr| tr.count > 0);
    let count_text = trackers.map(|tr| tr.count.min(999).to_string());
    let shield_width = count_text.as_ref().map(|text| 30.0 + ui.fonts_mut(|f| f.layout_no_wrap(text.clone(), semibold(12.0), t.text).size().x));
    let widths: Vec<f32> = [shield_width, source.then_some(BUTTON), Some(BUTTON + 10.0)].into_iter().flatten().collect();
    let width = widths.iter().sum::<f32>() + 2.0 * (widths.len().saturating_sub(1)) as f32 + 2.0 * PAD;
    // Clear of the scroll bar, wherever it is drawn (a floating one widens when pointed at).
    let scroll = &ui.spacing().scroll;
    let bar = scroll.bar_width.max(scroll.floating_allocated_width) + scroll.bar_inner_margin + scroll.bar_outer_margin;
    if room.width() < width + bar + 2.0 * INSET || room.height() < HEIGHT + 2.0 * INSET {
        return;
    }
    let rect = Rect::from_min_size(pos2(room.max.x - bar - INSET - width, room.min.y + INSET), vec2(width, HEIGHT + 2.0 * PAD));
    let painter = ui.painter().clone();
    painter.add(egui::epaint::Shadow { offset: [0, 2], blur: 10, spread: 0, color: t.shadow_popup }.as_shape(rect, 8.0));
    painter.rect(rect, 8.0, t.menu, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let mut bar_ui = ui.new_child(UiBuilder::new().max_rect(rect.shrink(PAD)).layout(Layout::left_to_right(egui::Align::Center)));
    bar_ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
    let ui = &mut bar_ui;
    if let (Some(tr), Some(text)) = (trackers, count_text) {
        shield(app, ui, m, place, tr, &text, shield_width.unwrap_or(BUTTON), &t);
    }
    if source {
        let on = app.source_pane.showing(m.id);
        let (label, tip) = if on {
            (tl!("Source"), tl!("Back to the message as it is meant to be read"))
        } else {
            (tl!("Source"), tl!("Show the message's source in place: text only, nothing runs or downloads"))
        };
        let response = tool(ui, BUTTON, on, label, tip, &t, |p, r, c| icons::paint(p, r, "formatHtml", c, c));
        app.hit.add("sourceToggle", m.id, response.rect);
        if response.clicked() {
            let _ = crate::source_pane::command(app, &json!({"on": !on}));
        }
    }
    let response = tool(ui, BUTTON + 10.0, false, tl!("Message appearance"), tl!("Show this message light, dark or as the app is"), &t, |p, r, c| {
        let icon = Rect::from_min_size(pos2(r.min.x - 4.0, r.min.y), r.size());
        icons::paint(p, icon, "theme", c, c);
        caret(p, pos2(icon.max.x + 5.0, r.center().y), c);
    });
    app.hit.add("messageAppearance", m.id, response.rect);
    egui::Popup::menu(&response).align(egui::RectAlign::BOTTOM_END).show(|ui| {
        ui.label(egui::RichText::new(tl!("Message appearance")).font(semibold(12.0)).color(t.text_dim));
        appearance::controls(app, ui, place);
    });
}

/// The shield: how many trackers were kept out, and the way to let them in for this message.
#[allow(clippy::too_many_arguments)]
fn shield(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place, tr: Trackers, count: &str, width: f32, t: &Tokens) {
    let n = tr.count;
    let label = if tr.kept { tln!(n, "{n} tracker loaded", "{n} trackers loaded") } else { tln!(n, "{n} tracker blocked", "{n} trackers blocked") };
    let tip = if tr.kept {
        tl!("You loaded this message's tracking pictures: its sender can tell that you opened it.")
    } else {
        tl!("Tracking pictures tell the sender when you open a message. They were left out; nothing was downloaded for them.")
    };
    let hue = if tr.kept { t.orange } else { t.icon_green };
    let ink = t.text;
    let response = tool(ui, width, false, &label, tip, t, |p, r, _| {
        // The shield at the left of the control, the count after it.
        let icon = Rect::from_min_size(pos2(r.min.x - (width - BUTTON) / 2.0 + 4.0, r.min.y), r.size());
        paint_shield(p, icon, hue, tr.kept);
        p.text(pos2(icon.max.x + 4.0, r.center().y), Align2::LEFT_CENTER, count, semibold(12.0), ink);
    });
    if place == Place::Pane {
        app.hit.add("trackersShield", m.id, response.rect);
    }
    egui::Popup::menu(&response).align(egui::RectAlign::BOTTOM_END).show(|ui| {
        ui.set_max_width(280.0);
        ui.label(egui::RichText::new(&label).font(semibold(12.5)).color(t.text));
        ui.add(egui::Label::new(egui::RichText::new(tip).font(regular(11.5)).color(t.text_dim)).wrap());
        if !tr.kept {
            ui.add_space(4.0);
            let load = ui.button(tl!("Load anyway")).on_hover_text(tl!("Download this message's tracking pictures too, until MailCraft closes"));
            if place == Place::Pane {
                app.hit.add("loadTrackers", m.id, load.rect);
            }
            if load.clicked() {
                view::keep_trackers(ui.ctx(), m);
                ui.close();
            }
        }
    });
}

/// A shield of `hue`, with a tick when the trackers were kept out, a dot when they were let in.
fn paint_shield(p: &egui::Painter, r: Rect, hue: Color32, kept: bool) {
    let s = r.width().min(r.height()) / 20.0;
    let at = |x: f32, y: f32| pos2(r.min.x + x * s, r.min.y + y * s);
    let outline = vec![at(10.0, 2.0), at(16.5, 4.5), at(16.0, 11.0), at(10.0, 18.0), at(4.0, 11.0), at(3.5, 4.5)];
    p.add(Shape::convex_polygon(outline, hue.gamma_multiply(0.18), Stroke::new(1.4, hue)));
    if kept {
        p.circle_filled(at(10.0, 9.5), 2.0 * s, hue);
    } else {
        p.line(vec![at(7.0, 9.8), at(9.3, 12.2), at(13.2, 7.5)], Stroke::new(1.6, hue));
    }
}

/// A small downward caret centred on `at`.
fn caret(p: &egui::Painter, at: egui::Pos2, c: Color32) {
    p.line(vec![pos2(at.x - 3.0, at.y - 1.5), pos2(at.x, at.y + 1.5), pos2(at.x + 3.0, at.y - 1.5)], Stroke::new(1.3, c));
}

/// One control of the bar: `width` wide, its look painted by `draw` into a 16-point square at
/// its centre, `selected` when it is a toggle that is on. Named `label` for screen readers.
fn tool(ui: &mut Ui, width: f32, selected: bool, label: &str, tip: &str, t: &Tokens, draw: impl FnOnce(&egui::Painter, Rect, Color32)) -> Response {
    let (r, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        t.pressed
    } else if selected {
        t.checked
    } else if response.hovered() {
        t.hover
    } else {
        Color32::TRANSPARENT
    };
    let p = ui.painter();
    p.rect_filled(r, 5.0, fill);
    if response.has_focus() {
        p.rect_stroke(r.shrink(1.0), 5.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
    let ink = if selected { t.accent_text } else { t.icon };
    draw(p, Rect::from_center_size(r.center(), vec2(16.0, 16.0)), ink);
    let label = label.to_string();
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &label));
    response.on_hover_text(tip)
}
