// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an agent asks to send, shown to the owner as it will go: the message Send builds from
//! what is being written (`mailcraft_engine::cmd::compose::preview_of`), with its header, body
//! and a list of its files, read-only and asking nothing of the network. The question about
//! sending shows it under its words, in the window and in a headless host's question window
//! alike, so the owner's yes is about the message itself and not only its subject.
//!
//! A yes still reaches only the message as it was when asked (`agent::bind`): what is shown here
//! is that same message.

use std::sync::Arc;

use egui::{Ui, vec2};
use mailcraft_model::{Mailbox, Message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::MailApp;
use crate::reading::{self, Place};
use crate::theme::{self, Tokens, regular};

/// Most bytes of pictures shown in the body (`cid:`) that go with a preview to a question window;
/// past it they are left out and shown as missing pictures.
const MOST_PICTURE_BYTES: usize = 8 << 20;

/// A file the message carries: its name, type and size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub mime: String,
    pub size: u64,
}

/// The message as Send will build it, and its files apart (their bytes aren't needed to show
/// them, and a file is never opened from here).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendPreview {
    pub message: Arc<Message>,
    pub files: Vec<File>,
}

impl SendPreview {
    /// The preview of what `compose.send` with `params` would send, if there is a message being
    /// written that it names and Send could build. `None` for any other command.
    pub(crate) fn of(app: &MailApp, id: &str, params: &Value) -> Option<SendPreview> {
        if id != "compose.send" {
            return None;
        }
        let compose = app.session.compose_named(params)?;
        let message = mailcraft_engine::cmd::compose::preview_of(&app.session, compose).ok()?;
        Some(SendPreview::from_message(message))
    }

    /// Split a built message into what is shown in its body and the files it carries.
    pub fn from_message(mut message: Message) -> SendPreview {
        let mut files = Vec::new();
        let mut pictures = 0usize;
        message.attachments.retain_mut(|a| {
            if a.inline && !a.content_id.is_empty() {
                pictures = pictures.saturating_add(a.data.len());
                if pictures > MOST_PICTURE_BYTES {
                    a.data = Arc::new(Vec::new());
                }
                return true;
            }
            files.push(File { name: a.name.clone(), mime: a.mime.clone(), size: a.size() as u64 });
            false
        });
        // A signed or encrypted message's original isn't part of what is shown.
        message.original = None;
        SendPreview { message: Arc::new(message), files }
    }

    /// The files, as a line: `a.pdf (12 KB), b.txt (3 KB)`.
    pub fn files_line(&self) -> String {
        self.files
            .iter()
            .map(|f| format!("{} ({})", f.name.chars().filter(|c| !c.is_control()).collect::<String>(), mailcraft_model::size_label(f.size)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Show `preview` in a box `height` points high and as wide as `ui` is: the files first, then the
/// message as a recipient reads it (Bcc included, as the sender's copy has it). Pictures on the web
/// are never fetched for it, and nothing in it can be opened.
pub(crate) fn show(app: &mut MailApp, ui: &mut Ui, preview: &SendPreview, height: f32) {
    let t = Tokens::get(ui.ctx());
    let width = ui.available_width();
    egui::Frame::NONE.stroke(egui::Stroke::new(1.0, t.border_strong)).corner_radius(4.0).show(ui, |ui| {
        ui.set_width(width - 2.0);
        if !preview.files.is_empty() {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 0 }).show(ui, |ui| {
                let line = crate::i18n::fmt(tl!("Attached: {files}"), &[("files", &preview.files_line())]);
                ui.add(egui::Label::new(egui::RichText::new(line).font(regular(12.0)).color(t.text)).wrap());
            });
        }
        ui.allocate_ui(vec2(width - 2.0, height.max(120.0)), |ui| {
            ui.set_min_height(height.max(120.0));
            reading::show_file_message(app, ui, &preview.message, Place::Preview);
        });
    });
}

/// A headless host's question window shows a send question's preview with this: an app of its
/// own over an empty mailbox, which opens no file, saves nothing and starts nothing.
pub struct PreviewWindow {
    app: MailApp,
    preview: SendPreview,
}

impl PreviewWindow {
    /// For the question window's context: its fonts and theme are installed here.
    pub fn new(ctx: &egui::Context, preview: SendPreview) -> PreviewWindow {
        theme::install_fonts(ctx);
        let choice = theme::ThemeChoice { appearance: theme::Appearance::default(), accent: theme::Accent::default(), high_contrast: false };
        theme::apply(ctx, choice.appearance.preference(theme::main_system_theme(ctx)), &choice);
        let session = mailcraft_engine::Session::at(Mailbox::default(), mailcraft_time::now(), 0);
        let mut app = MailApp::new(session, crate::Services::default());
        app.autosave = false;
        PreviewWindow { app, preview }
    }

    /// Show it in `height` points of `ui`.
    pub fn show(&mut self, ui: &mut Ui, height: f32) {
        show(&mut self.app, ui, &self.preview, height);
    }
}
