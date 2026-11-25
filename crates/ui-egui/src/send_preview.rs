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

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{Mailbox, Message};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::reading::{self, Place};
use crate::theme::{self, Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

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
        // The files, plain to see over the message (v0.12.14): how many, and each by its name and size.
        if !preview.files.is_empty() {
            egui::Frame::NONE.fill(t.ribbon).inner_margin(egui::Margin { left: 12, right: 12, top: 8, bottom: 10 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                let n = preview.files.len();
                let heading = ui.label(egui::RichText::new(tln!(n, "{n} attachment", "{n} attachments")).font(semibold(13.0)).color(t.text));
                app.hit.add("sendPreviewAttachments", 0, heading.rect);
                ui.horizontal_wrapped(|ui| {
                    for (index, file) in preview.files.iter().enumerate() {
                        let chip = file_chip(ui, file, &t);
                        app.hit.add("sendPreviewAttachment", index as u64, chip);
                    }
                });
            });
        }
        ui.allocate_ui(vec2(width - 2.0, height.max(120.0)), |ui| {
            ui.set_min_height(height.max(120.0));
            reading::show_file_message(app, ui, &preview.message, Place::Preview);
        });
    });
}

/// A file the message carries, as a chip like the reading pane's: its name and its size under
/// it. It only shows: nothing opens it. Answers where it is.
fn file_chip(ui: &mut Ui, file: &File, t: &Tokens) -> Rect {
    let name: String = mailcraft_engine::opening::file_name(&file.name).chars().filter(|c| !c.is_control()).collect();
    let size = mailcraft_model::size_label_in(crate::i18n::current(), file.size);
    let name_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(name.clone(), regular(12.0), t.text).size().x).min(220.0);
    let size_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(size.clone(), regular(10.5), t.text_dim).size().x);
    let w = name_w.max(size_w);
    let (r, response) = ui.allocate_exact_size(vec2(w + 44.0, 38.0), Sense::hover());
    ui.painter().rect(r, 5.0, t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 17.0, r.center().y), vec2(20.0, 20.0)), "attach", t.icon, t.accent);
    paint_clipped(ui.painter(), pos2(r.min.x + 34.0, r.min.y + 13.0), w + 4.0, &name, regular(12.0), t.text);
    ui.painter().text(pos2(r.min.x + 34.0, r.min.y + 27.0), Align2::LEFT_CENTER, &size, regular(10.5), t.text_dim);
    let said = format!("{name} ({size})");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &said));
    response.on_hover_text(said);
    r
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
        let choice = theme::ThemeChoice {
            appearance: theme::Appearance::default(),
            accent: theme::Accent::default(),
            custom: None,
            high_contrast: false,
            dark_style: Default::default(),
        };
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
