// SPDX-FileCopyrightText: The winit contributors
// SPDX-License-Identifier: Apache-2.0
use super::*;

fn regions(scale: f64) -> CaptionRegions {
    CaptionRegions {
        client_size: PhysicalSize::new(1200, 800),
        scale_factor: scale,
        drag: vec![CaptionRect { left: 200, top: 8, right: 900, bottom: 48 }],
        maximize: Some(CaptionRect { left: 1100, top: 8, right: 1146, bottom: 48 }),
        system_menu: Some(CaptionRect { left: 8, top: 8, right: 40, bottom: 48 }),
        exclusions: vec![CaptionRect { left: 400, top: 0, right: 600, bottom: 200 }],
    }
}

#[test]
fn controls_popups_and_drag_have_distinct_native_hits() {
    let r = regions(1.0);
    let hit = |x,y| r.hit(x,y,r.client_size,1.0,true,false);
    assert_eq!(hit(300,24),CaptionHit::Drag);
    assert_eq!(hit(500,24),CaptionHit::Client);
    assert_eq!(hit(1110,24),CaptionHit::Maximize);
    assert_eq!(hit(20,24),CaptionHit::SystemMenu);
    assert_eq!(hit(100,24),CaptionHit::Client);
}

#[test]
fn resizing_and_dpi_changes_never_reuse_stale_button_geometry() {
    let r=regions(1.0);
    assert_eq!(r.hit(1110,24,PhysicalSize::new(1300,800),1.0,true,false),CaptionHit::Client);
    assert_eq!(r.hit(1110,24,r.client_size,1.5,true,false),CaptionHit::Client);
    assert_eq!(r.hit(1,1,r.client_size,1.5,true,false),CaptionHit::TopLeft);
    assert_eq!(r.hit(1,1,r.client_size,1.0,true,true),CaptionHit::Client);
    assert_eq!(r.hit(-1,24,r.client_size,1.0,true,false),CaptionHit::Client);
}

#[test]
fn custom_windows_dpi_uses_the_renderers_native_precision() {
    let scale=125.0/96.0;
    let r=regions(f64::from(scale as f32));
    assert_eq!(r.hit(1110,24,r.client_size,scale,true,false),CaptionHit::Maximize);
}
