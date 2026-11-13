// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The headless question window's layout, time and answers.

use super::*;

fn window(seconds: u64) -> Window {
    let shown = Shown {
        title: "Send?".into(),
        text: "send “a” to b@example.com".into(),
        note: "Say yes only if you asked the agent for this.".into(),
        yes: "Send".into(),
        no: "Don't Send".into(),
        seconds,
    };
    Window { shown, said: Arc::new(Mutex::new(Said::Closed)), until: None, preview: None }
}

/// One frame: where the question was shown, and whether the window asked to close.
fn frame_closing(ctx: &egui::Context, window: &mut Window, events: Vec<egui::Event>) -> (egui::Rect, bool) {
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(780.0, 720.0));
    let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
    let mut question = egui::Rect::NOTHING;
    let mut output = ctx.run_ui(input, |ui| question = window.show(ui));
    output.textures_delta.clear();
    let closing = output.viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)));
    (question, closing)
}

fn frame(ctx: &egui::Context, window: &mut Window, events: Vec<egui::Event>) -> egui::Rect {
    frame_closing(ctx, window, events).0
}

/// The answers stay a row at the bottom, frame after frame: the question above them keeps the
/// rest of the window (they once grew until they filled it and the question was gone).
#[test]
fn the_answers_never_take_the_question_s_room() {
    let ctx = egui::Context::default();
    let mut window = window(3600);
    let mut question = egui::Rect::NOTHING;
    for _ in 0..20 {
        question = frame(&ctx, &mut window, Vec::new());
    }
    assert!(question.height() > 600.0, "the question was left {question:?} of 720 points");
}

/// The window's time starts at its first frame on the screen: however long it took to start,
/// none of it is taken from the owner.
#[test]
fn the_time_starts_when_the_window_is_shown() {
    let ctx = egui::Context::default();
    let mut window = window(60);
    assert!(window.until.is_none());
    std::thread::sleep(Duration::from_millis(300));
    let before = Instant::now();
    frame(&ctx, &mut window, Vec::new());
    let until = window.until.expect("the first frame starts the time");
    assert!(until >= before + Duration::from_secs(60), "the time started before the window was shown");
    // Later frames don't start it again.
    frame(&ctx, &mut window, Vec::new());
    assert_eq!(window.until, Some(until));
}

/// No key answers: Enter, Space and Escape typed while the window is up (for something else, or
/// at it) leave the question open and unanswered.
#[test]
fn keys_never_answer() {
    let ctx = egui::Context::default();
    let mut window = window(3600);
    frame(&ctx, &mut window, Vec::new());
    for key in [egui::Key::Enter, egui::Key::Space, egui::Key::Escape, egui::Key::Tab, egui::Key::Enter] {
        let press = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE };
        let (_, closing) = frame_closing(&ctx, &mut window, vec![press(true), press(false)]);
        assert!(!closing, "{key:?} closed the question");
        let (_, closing) = frame_closing(&ctx, &mut window, Vec::new());
        assert!(!closing, "{key:?} closed the question a frame later");
    }
    assert_eq!(*window.said.lock().unwrap(), Said::Closed);
}

/// A question's time is at least a minute and at most a day; none given is no window at all.
#[test]
fn a_question_stays_up_a_minute_at_least() {
    assert_eq!(seconds_of(&json!({"seconds": 2})), LEAST_SECONDS);
    assert_eq!(seconds_of(&json!({"seconds": 600})), 600);
    assert_eq!(seconds_of(&json!({"seconds": 10_000_000})), MOST_SECONDS);
    assert_eq!(seconds_of(&json!({"seconds": 0})), 0);
    assert_eq!(seconds_of(&json!({})), 0);
}
