// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Publish rendered egui caption rectangles through winit's safe Windows extension.

use mailcraft_ui_egui::MailApp;
use winit::platform::windows::{CaptionRect, CaptionRegions, WindowExtWindows};

fn pixels(rect: egui::Rect, scale: f32) -> CaptionRect {
    CaptionRect {
        left: (rect.left() * scale).floor() as i32,
        top: (rect.top() * scale).floor() as i32,
        right: (rect.right() * scale).ceil() as i32,
        bottom: (rect.bottom() * scale).ceil() as i32,
    }
}

pub fn publish(app: &mut MailApp, ctx: &egui::Context, window: &winit::window::Window) {
    if !app.windows_titlebar {
        window.set_caption_regions(None);
        return;
    }
    app.window_header.exclude_overlays(ctx);
    let scale = ctx.pixels_per_point();
    if !scale.is_finite() || scale <= 0.0 {
        window.set_caption_regions(None);
        return;
    }
    let header = &app.window_header;
    let drawn = ctx.input(|input| input.viewport_rect().size());
    window.set_caption_regions(Some(CaptionRegions {
        client_size: winit::dpi::PhysicalSize::new((drawn.x * scale).round() as u32, (drawn.y * scale).round() as u32),
        scale_factor: ctx.native_pixels_per_point().map(f64::from).unwrap_or_else(|| window.scale_factor()),
        drag: header.drag.iter().map(|r| pixels(*r, scale)).collect(),
        maximize: header.maximize.map(|r| pixels(r, scale)),
        system_menu: header.system_menu.map(|r| pixels(r, scale)),
        exclusions: header.exclusions.iter().map(|r| pixels(*r, scale)).collect(),
    }));
}
