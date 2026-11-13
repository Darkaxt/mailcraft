// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The headless question window's layout.

use super::*;

/// The answers stay a row at the bottom, frame after frame: the question above them keeps the
/// rest of the window (they once grew until they filled it and the question was gone).
#[test]
fn the_answers_never_take_the_question_s_room() {
    let ctx = egui::Context::default();
    let shown = Shown {
        title: "Send?".into(),
        text: "send “a” to b@example.com".into(),
        note: "Say yes only if you asked the agent for this.".into(),
        yes: "Send".into(),
        no: "Don't Send".into(),
        until: Instant::now() + Duration::from_secs(3600),
    };
    let mut window = Window { shown, said: Arc::new(Mutex::new(false)), focused: false, preview: None };
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(780.0, 720.0));
    let mut question = egui::Rect::NOTHING;
    for _ in 0..20 {
        let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
        ctx.run_ui(input, |ui| question = window.show(ui)).textures_delta.clear();
    }
    assert!(question.height() > 600.0, "the question was left {question:?} of 720 points");
}
