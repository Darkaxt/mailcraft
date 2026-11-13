// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Headless MailCraft asks the owner about what an agent wants in a small window of its own
//! (`mailcraft_ui_egui::Services::ask_owner`): a separate, short-lived copy of this program,
//! started with [`ARG`], that reads the question from its standard input, shows it on top of
//! everything and ends with exit code 0 only when the owner clicked the yes button. Clicking no,
//! closing it, its time running out, and anything going wrong are a no. It takes no keyboard
//! focus when it comes up and no key answers it: what the owner was typing elsewhere when it
//! appeared never answers for them. Its time is counted from its first frame on the screen, not
//! from its start. It says on its standard output how it ended, for the host's log. That copy
//! opens no mailbox, takes no single-instance lock, starts no control port, writes no settings
//! and logs nothing: it is started before any of that in `main`.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_ui_egui::{OwnerQuestion, PreviewWindow, SendPreview};
use serde_json::{Value, json};

/// The argument that starts the question window instead of MailCraft.
pub const ARG: &str = "--owner-question";

/// Most bytes of a question read from standard input: a send's message comes with it, its
/// pictures included (its files are only named).
const MOST_BYTES: u64 = 32 << 20;

/// Most seconds a question window stays up, whatever it is told: a day, so that an owner who is
/// away still finds it (`agent_gate::HEADLESS_WAIT_MS`).
const MOST_SECONDS: u64 = 86_400;

/// Fewest seconds a question window stays up, whatever it is told: a minute to read and decide.
const LEAST_SECONDS: u64 = 60;

/// How long the host waits for the window past its time before ending it: the window's time only
/// starts once it is on the screen, and starting it (graphics, fonts, the message) takes a while.
const STARTUP_SLACK_SECONDS: u64 = 120;

/// Below this many seconds left, the window shows how long it has.
const COUNTDOWN_SECONDS: u64 = 600;

/// The host's `ask_owner`: each question in a window process of its own, whose answer is sent
/// and the app woken when it has gone.
pub fn hook(ctx: egui::Context) -> Box<dyn Fn(OwnerQuestion) -> Receiver<bool>> {
    Box::new(move |question| {
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        let spawned = std::thread::Builder::new().name("owner question".into()).spawn(move || {
            let yes = ask(&question);
            let _ = tx.send(yes);
            wake.request_repaint();
        });
        if let Err(why) = spawned {
            // The sender went with the closure: the question is a no.
            log::warn!("the owner question window couldn't be started: {why}");
        }
        rx
    })
}

/// Show `question` in a window process and wait for it: `true` only on its exit code 0.
fn ask(question: &OwnerQuestion) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        log::warn!("the owner question window couldn't be started: this program's path is unknown");
        return false;
    };
    let mut child = match Command::new(exe).arg(ARG).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(why) => {
            log::warn!("the owner question window couldn't be started: {why}");
            return false;
        }
    };
    let seconds = question.seconds.min(MOST_SECONDS);
    if seconds == 0 {
        log::info!("owner question: no time was left to ask; it is a no");
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    let body = json!({
        "title": question.title,
        "text": question.text,
        "note": question.note,
        "yes": question.yes,
        "no": question.no,
        "seconds": seconds,
        "preview": question.preview,
    });
    let written = child.stdin.take().map(|mut stdin| stdin.write_all(body.to_string().as_bytes()).is_ok()).unwrap_or(false);
    // (Standard input is closed here, so the window reads to its end.)
    if !written {
        let _ = child.kill();
        let _ = child.wait();
        return false;
    }
    // The window closes itself when its time is up, counted from when it was first on the screen;
    // past that and the time it may take to start, it is ended.
    let deadline = Instant::now() + Duration::from_secs(seconds.max(LEAST_SECONDS) + STARTUP_SLACK_SECONDS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // How it ended, as the window said it (a few words, never what was asked).
                let mut said = String::new();
                if let Some(out) = child.stdout.take() {
                    let _ = out.take(256).read_to_string(&mut said);
                }
                let said: String = said.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ').take(64).collect();
                let said = if said.trim().is_empty() { "nothing said" } else { said.trim() };
                log::info!("owner question window: {said} ({status})");
                return status.success();
            }
            Ok(None) if question.cancel.load(Ordering::Relaxed) || Instant::now() >= deadline => {
                let why = if question.cancel.load(Ordering::Relaxed) { "the question went" } else { "it didn't close in time" };
                log::info!("owner question window ended by the host: {why}");
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(why) => {
                log::warn!("the owner question window couldn't be waited for: {why}");
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// The question as the window shows it.
struct Shown {
    title: String,
    text: String,
    note: String,
    yes: String,
    no: String,
    /// How long it stays up, from its first frame on the screen.
    seconds: u64,
}

/// How long a question window is told to stay up, within a minute and a day; 0 when it wasn't
/// told (that is a no, never a window).
fn seconds_of(v: &Value) -> u64 {
    match v.get("seconds").and_then(Value::as_u64).unwrap_or(0) {
        0 => 0,
        n => n.clamp(LEAST_SECONDS, MOST_SECONDS),
    }
}

/// How the window ended, for the host's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Said {
    Yes,
    No,
    Closed,
    TimedOut,
}

impl Said {
    fn word(self) -> &'static str {
        match self {
            Said::Yes => "yes",
            Said::No => "no clicked",
            Said::Closed => "closed",
            Said::TimedOut => "timed out",
        }
    }
}

/// The window process: the exit code, 0 only for the owner's yes.
pub fn window() -> i32 {
    let mut input = Vec::new();
    if std::io::stdin().lock().take(MOST_BYTES).read_to_end(&mut input).is_err() {
        return 1;
    }
    let Ok(v) = serde_json::from_slice::<Value>(&input) else { return 1 };
    let text = |k: &str| -> String { v.get(k).and_then(Value::as_str).unwrap_or("").chars().filter(|c| !c.is_control()).collect() };
    let seconds = seconds_of(&v);
    let (yes, no) = (text("yes"), text("no"));
    if seconds == 0 || yes.is_empty() || no.is_empty() {
        return 1;
    }
    let shown = Shown { title: text("title"), text: text("text"), note: text("note"), yes, no, seconds };
    // A send's message, as it will go: shown under the question. One that can't be read is a no,
    // never a question without it.
    let preview = match v.get("preview") {
        None | Some(Value::Null) => None,
        Some(p) => match serde_json::from_value::<SendPreview>(p.clone()) {
            Ok(p) => Some(p),
            Err(_) => return 1,
        },
    };
    let size = if preview.is_some() { [780.0, 720.0] } else { [440.0, 220.0] };
    let said = Arc::new(Mutex::new(Said::Closed));
    let answer = said.clone();
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MailCraft — an agent is asking")
            .with_inner_size(size)
            .with_resizable(preview.is_some())
            .with_always_on_top()
            // Not given the keyboard: what is being typed elsewhere stays there.
            .with_active(false)
            .with_minimize_button(false)
            .with_maximize_button(false),
        ..Default::default()
    };
    #[cfg(target_os = "windows")]
    crate::graphics::configure(&mut options, eframe::wgpu::Backends::from_env());
    let started = eframe::run_native(
        "MailCraft owner question",
        options,
        Box::new(move |cc| {
            let preview = preview.map(|p| PreviewWindow::new(&cc.egui_ctx, p));
            Ok(Box::new(Window { shown, said: answer, until: None, preview }) as Box<dyn eframe::App>)
        }),
    );
    let said = *said.lock().unwrap_or_else(|e| e.into_inner());
    let word = match &started {
        Ok(()) => said.word().to_string(),
        Err(why) => format!("not shown {}", why.to_string().chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ').take(40).collect::<String>()),
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{word}");
    let _ = out.flush();
    if started.is_ok() && said == Said::Yes { 0 } else { 1 }
}

struct Window {
    shown: Shown,
    said: Arc<Mutex<Said>>,
    /// When its time is up: set at its first frame, so starting the window takes none of it.
    until: Option<Instant>,
    /// A send's message ([`SendPreview`]).
    preview: Option<PreviewWindow>,
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl Window {
    /// The window's one frame. Returns where the question and the message were shown, so a test
    /// can see they keep their room.
    fn show(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let ctx = ui.ctx().clone();
        let seconds = self.shown.seconds;
        let until = *self.until.get_or_insert_with(|| Instant::now() + Duration::from_secs(seconds));
        let left = until.saturating_duration_since(Instant::now());
        // The time running out: no (closing the window is a no by itself). No key answers.
        if left.is_zero() {
            *self.said.lock().unwrap_or_else(|e| e.into_inner()) = Said::TimedOut;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return egui::Rect::NOTHING;
        }
        // The note, the time left and the answers are kept at the bottom, always in sight however
        // long the message shown above them is. The panel is as high as what is in it and no
        // higher: not resizable (a resizable panel keeps the most it was ever given), and the
        // buttons in a row of their own height (a layout centred across the panel's whole height
        // would grow it every frame until it filled the window and hid the question).
        egui::Panel::bottom("answer").resizable(false).show_separator_line(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            ui.add_space(4.0);
            ui.label(egui::RichText::new(self.shown.note.as_str()).weak());
            if left.as_secs() < COUNTDOWN_SECONDS {
                ui.label(egui::RichText::new(format!("{} s", left.as_secs())).weak());
            }
            let row = egui::vec2(ui.available_width(), 30.0);
            ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Answered with the pointer only: neither button keeps the keyboard's focus, so
                // Enter or Space typed for something else can't press one.
                let no = ui.add(egui::Button::new(self.shown.no.as_str()).min_size(egui::vec2(88.0, 26.0)));
                let yes = ui.add(egui::Button::new(self.shown.yes.as_str()).min_size(egui::vec2(72.0, 26.0)));
                for button in [&no, &yes] {
                    if button.has_focus() {
                        button.surrender_focus();
                    }
                }
                let said = if yes.clicked_by(egui::PointerButton::Primary) {
                    Some(Said::Yes)
                } else if no.clicked_by(egui::PointerButton::Primary) {
                    Some(Said::No)
                } else {
                    None
                };
                if let Some(said) = said {
                    *self.said.lock().unwrap_or_else(|e| e.into_inner()) = said;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.add_space(4.0);
        });
        let shown = egui::CentralPanel::default().show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
            // Plain text only: what an agent asked about is never markup or a link here.
            ui.label(egui::RichText::new(self.shown.title.as_str()).strong());
            ui.label(self.shown.text.as_str());
            if let Some(preview) = self.preview.as_mut() {
                // The rest of the window, above the answers.
                let height = (ui.available_height() - 8.0).max(120.0);
                preview.show(ui, height);
            }
        });
        ctx.request_repaint_after(Duration::from_millis(250));
        shown.response.rect
    }
}

#[cfg(test)]
#[path = "owner_question_tests.rs"]
mod owner_question_tests;
