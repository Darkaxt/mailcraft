// SPDX-FileCopyrightText: The winit contributors
// SPDX-License-Identifier: Apache-2.0
//! Client-area caption geometry. Native dispatch stays in the existing Win32 procedure.

use crate::dpi::PhysicalSize;

/// A half-open rectangle in physical client pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptionRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl CaptionRect {
    fn contains(self, x: i32, y: i32) -> bool {
        self.left <= x && x < self.right && self.top <= y && y < self.bottom
    }
}

/// The last rendered caption, bound to that frame's native client extent and DPI.
/// Rectangles are data, never a callback into the renderer from a native procedure.
#[derive(Clone, Debug)]
pub struct CaptionRegions {
    pub client_size: PhysicalSize<u32>,
    pub scale_factor: f64,
    pub drag: Vec<CaptionRect>,
    pub maximize: Option<CaptionRect>,
    pub system_menu: Option<CaptionRect>,
    pub exclusions: Vec<CaptionRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaptionHit {
    Client,
    Drag,
    Maximize,
    SystemMenu,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl CaptionRegions {
    pub(crate) fn bounded(mut self) -> Self {
        if self.drag.len() > 32 || self.exclusions.len() > 64 {
            self.drag.clear();
            self.maximize = None;
            self.system_menu = None;
        }
        self.drag.truncate(32);
        self.exclusions.truncate(64);
        self
    }

    pub(crate) fn hit(
        &self,
        x: i32,
        y: i32,
        size: PhysicalSize<u32>,
        scale: f64,
        resizable: bool,
        maximized: bool,
    ) -> CaptionHit {
        let width = i32::try_from(size.width).unwrap_or(i32::MAX);
        let height = i32::try_from(size.height).unwrap_or(i32::MAX);
        if x < 0 || y < 0 || x >= width || y >= height || !scale.is_finite() || scale <= 0.0 {
            return CaptionHit::Client;
        }
        // Edges belong to Windows even before the next frame has caught up to a resize.
        if resizable && !maximized {
            let border = (8.0 * scale.clamp(0.25, 8.0)).round() as i32;
            let left = x < border;
            let right = x >= width.saturating_sub(border);
            let top = y < border;
            let bottom = y >= height.saturating_sub(border);
            let edge = match (left, right, top, bottom) {
                (true, _, true, _) => Some(CaptionHit::TopLeft),
                (_, true, true, _) => Some(CaptionHit::TopRight),
                (true, _, _, true) => Some(CaptionHit::BottomLeft),
                (_, true, _, true) => Some(CaptionHit::BottomRight),
                (true, _, _, _) => Some(CaptionHit::Left),
                (_, true, _, _) => Some(CaptionHit::Right),
                (_, _, true, _) => Some(CaptionHit::Top),
                (_, _, _, true) => Some(CaptionHit::Bottom),
                _ => None,
            };
            if let Some(edge) = edge {
                return edge;
            }
        }
        // A stale frame must never drag or maximize where a newly resized control now sits.
        // egui reports native DPI as f32. Compare that representation rather
        // than rejecting custom Windows DPI merely because f64 has more bits.
        if self.client_size != size || self.scale_factor as f32 != scale as f32 {
            return CaptionHit::Client;
        }
        if self.exclusions.iter().any(|rect| rect.contains(x, y)) {
            return CaptionHit::Client;
        }
        if resizable && self.maximize.is_some_and(|rect| rect.contains(x, y)) {
            return CaptionHit::Maximize;
        }
        if self.system_menu.is_some_and(|rect| rect.contains(x, y)) {
            return CaptionHit::SystemMenu;
        }
        if self.drag.iter().any(|rect| rect.contains(x, y)) {
            CaptionHit::Drag
        } else {
            CaptionHit::Client
        }
    }
}

#[cfg(test)]
#[path = "windows_caption_tests.rs"]
mod tests;
