// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rendered Windows caption geometry, accessible controls and normal window commands.

use crate::{MailApp, theme::Tokens};
use egui::{LayerId, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::{Value, json};

pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;

/// Last rendered geometry; the native host publishes a physical-pixel snapshot.
#[derive(Clone, Debug, Default)]
pub struct WindowHeader {
    pub drag: Vec<Rect>,
    pub maximize: Option<Rect>,
    pub system_menu: Option<Rect>,
    pub exclusions: Vec<Rect>,
    pub layer: Option<LayerId>,
}

impl WindowHeader {
    /// Menus and modal overlays win over native caption interaction too.
    pub fn exclude_overlays(&mut self, ctx: &egui::Context) {
        let Some(header) = self.layer else { return };
        let rects = ctx.memory(|memory| {
            memory
                .areas()
                .visible_layer_ids()
                .into_iter()
                .filter(|layer| layer.order > header.order)
                .filter_map(|layer| memory.area_rect(layer.id))
                .filter(|rect| {
                    self.drag.iter().any(|drag| rect.intersects(*drag))
                        || self.maximize.is_some_and(|button| rect.intersects(button))
                        || self.system_menu.is_some_and(|mark| rect.intersects(mark))
                })
                .take(65)
                .collect::<Vec<_>>()
        });
        if self.exclusions.len().saturating_add(rects.len()) > 64 {
            self.drag.clear();
            self.maximize = None;
            self.system_menu = None;
        } else {
            self.exclusions.extend(rects);
        }
    }
}

pub(crate) fn command(app: &mut MailApp, id: &str) -> Result<Value, String> {
    if app.is_headless() {
        return Err("This MailCraft instance has no desktop window".into());
    }
    // Win32 minimize/maximize uses ShowWindow before applying hidden flags. Never
    // expose a deliberately hidden native window even briefly through an MCP action.
    if app.lifecycle.window_is_hidden() && matches!(id, "ui.minimizeMainWindow" | "ui.toggleMaximizeMainWindow") {
        return Err("The main window is hidden; minimize/maximize would reveal it".into());
    }
    let Some(ctx) = app.ctx.as_ref() else { return Err("The desktop window is not available".into()) };
    let command = match id {
        "ui.minimizeMainWindow" => egui::ViewportCommand::Minimized(true),
        "ui.toggleMaximizeMainWindow" => egui::ViewportCommand::Maximized(!ctx.input(|input| input.viewport().maximized.unwrap_or(false))),
        // The native close path still asks about unsaved drafts and saves durably.
        "ui.closeMainWindow" => egui::ViewportCommand::Close,
        _ => return Err("Unknown window action".into()),
    };
    ctx.send_viewport_cmd(command);
    ctx.request_repaint();
    Ok(json!({"requested":true}))
}

/// Windows owns mouse maximize/Snap; keyboard activation uses the same normal command.
pub(crate) fn buttons(app: &mut MailApp, ui: &mut Ui) -> Option<Rect> {
    let t = Tokens::get(ui.ctx());
    let maximized = ui.ctx().input(|input| input.viewport().maximized.unwrap_or(false));
    let mut maximize_rect = None;
    let previous_spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    // Parent layout is right to left.
    for (kind, label, command) in [
        (2, tl!("Close"), "ui.closeMainWindow"),
        (1, if maximized { tl!("Restore") } else { tl!("Maximize") }, "ui.toggleMaximizeMainWindow"),
        (0, tl!("Minimize"), "ui.minimizeMainWindow"),
    ] {
        let (rect, response) = ui.allocate_exact_size(vec2(CAPTION_BUTTON_WIDTH, crate::chrome::TITLE_BAR), Sense::click());
        if response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 0.0, if response.is_pointer_button_down_on() { t.pressed } else { t.hover });
        }
        if response.has_focus() {
            ui.painter().rect_stroke(rect.shrink(3.0), 2.0, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
        }
        let center = rect.center();
        let stroke = Stroke::new(1.0, t.icon);
        match kind {
            0 => {
                ui.painter().line_segment([center + vec2(-5.0, 0.0), center + vec2(5.0, 0.0)], stroke);
            }
            1 if maximized => {
                let front = Rect::from_center_size(center + vec2(-1.5, 1.5), vec2(8.0, 8.0));
                let back = front.translate(vec2(3.0, -3.0));
                ui.painter().line_segment([back.left_top(), back.right_top()], stroke);
                ui.painter().line_segment([back.right_top(), back.right_bottom()], stroke);
                ui.painter().rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
            }
            1 => {
                ui.painter().rect_stroke(Rect::from_center_size(center, vec2(10.0, 10.0)), 0.0, stroke, egui::StrokeKind::Inside);
            }
            _ => {
                ui.painter().line_segment([center + vec2(-5.0, -5.0), center + vec2(5.0, 5.0)], stroke);
                ui.painter().line_segment([center + vec2(-5.0, 5.0), center + vec2(5.0, -5.0)], stroke);
            }
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        app.hit.add("windowCaption", kind, rect);
        if kind == 1 {
            maximize_rect = Some(rect);
        }
        if response.on_hover_text(label).clicked() {
            let _ = app.run(command, json!({}));
        }
    }
    ui.spacing_mut().item_spacing.x = previous_spacing;
    maximize_rect
}

pub(crate) fn drag_strip(full: Rect, left: f32, right: f32) -> Vec<Rect> {
    if right > left { vec![Rect::from_min_max(pos2(left, full.top()), pos2(right, full.bottom()))] } else { Vec::new() }
}

#[cfg(test)]
#[path = "window_chrome_tests.rs"]
mod tests;
