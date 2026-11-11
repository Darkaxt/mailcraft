// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! In-place, selectable source. Only bounded pages live in the UI; the engine
//! owns preparation, integrity checking and the lifetime of native snapshots.

use std::sync::Arc;

use egui::{Ui, vec2};
use mailcraft_model::Message;
use serde_json::{Value, json};

use crate::{MailApp, theme::Tokens};

const PAGE: usize = 32 << 10;

#[derive(Default)]
pub(crate) struct Pane {
    active: Option<Active>,
    retiring: Vec<(u64, &'static str, u64)>,
}

struct Active {
    id: u64,
    opening: u64,
    /// The version the source was read from; replaced by flag or sync updates.
    message: Arc<Message>,
    body: Reader,
    export: Option<Reader>,
}

struct Reader {
    command: &'static str,
    token: Option<u64>,
    offset: u64,
    previous: Vec<u64>,
    answer: Option<Value>,
    wanted: bool,
    downloading: bool,
    error: Option<String>,
}

impl Reader {
    fn new(command: &'static str) -> Self {
        Self { command, token: None, offset: 0, previous: Vec::new(), answer: None, wanted: true, downloading: false, error: None }
    }

    fn step(&mut self, app: &mut MailApp, id: u64) {
        if !self.wanted || self.error.is_some() {
            return;
        }
        if self.downloading {
            if app.session.mailbox.message(id).is_some_and(|m| m.unfetched) {
                if !app.fetching.busy() {
                    self.error = Some(tl!("The complete message could not be downloaded. Check Send/Receive before trying again.").into());
                }
                return;
            }
            self.downloading = false;
        }
        // One preparation worker serves both body and export. Waiting for it is
        // different from replaying a request whose token is already known.
        if self.token.is_none() && app.session.source_export_busy() {
            return;
        }
        let params = match self.token {
            Some(token) => json!({"id":id,"token":token,"offset":self.offset,"limit":PAGE}),
            None => json!({"id":id,"limit":PAGE}),
        };
        match app.execute(self.command, params) {
            Err(why) => self.error = Some(why),
            Ok(value) => {
                if value.get("pending").and_then(Value::as_str) == Some("download") {
                    self.downloading = true;
                    return;
                }
                if let Some(token) = value.get("token").and_then(Value::as_u64) {
                    self.token = Some(token);
                }
                match value.get("state").and_then(Value::as_str) {
                    Some("pending") => (),
                    Some("ready") => {
                        self.answer = Some(value);
                        self.wanted = false;
                    }
                    _ => self.error = Some(tl!("The source reader is closed. Switch to Message and open Source again.").into()),
                }
            }
        }
    }
}

impl Pane {
    pub(crate) fn showing(&self, id: u64) -> bool {
        self.active.as_ref().is_some_and(|active| active.id == id)
    }

    fn retire(&mut self, active: Active) {
        for reader in std::iter::once(active.body).chain(active.export) {
            if let Some(token) = reader.token {
                self.retiring.push((active.id, reader.command, token));
            }
        }
    }

    pub(crate) fn inspect(&self) -> Value {
        self.active.as_ref().map_or(Value::Null, |active| {
            json!({
                "id":active.id,"mode":"source","offset":active.body.offset,
                "state":if active.body.error.is_some(){"failed"}else if active.body.wanted{"pending"}else{"ready"},
                "format":active.body.answer.as_ref().and_then(|v|v.get("format")),
                "nextOffset":active.body.answer.as_ref().and_then(|v|v.get("nextOffset")),
                "bytes":active.body.answer.as_ref().and_then(|v|v.get("bytes")),
                "exporting":active.export.is_some(),"error":active.body.error
            })
        })
    }
}

/// App-owned command, usable by the authenticated UI bridge without raw input.
/// Headless callers use mail.sourcePage/mail.exportEml instead.
pub(crate) fn command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, params);
        Err("The in-place source reader requires the native app. View Source remains available here.".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let object = params.as_object().ok_or("expected source options")?;
        if object.keys().any(|key| !matches!(key.as_str(), "on" | "page" | "export")) {
            return Err("unknown source option".into());
        }
        let on = match params.get("on") {
            Some(value) => Some(value.as_bool().ok_or("on must be a boolean")?),
            None => None,
        };
        let export = match params.get("export") {
            Some(value) => value.as_bool().ok_or("export must be a boolean")?,
            None => false,
        };
        let page = match params.get("page") {
            Some(value) => Some(value.as_str().filter(|page| matches!(*page, "next" | "previous")).ok_or("page must be next or previous")?),
            None => None,
        };
        if on == Some(false) && (export || page.is_some()) {
            return Err("open Source before paging or exporting".into());
        }
        let id = app.session.sel.focus().filter(|_| app.session.sel.messages.len() == 1).ok_or("select one message first")?;
        if on == Some(false) {
            if let Some(active) = app.source_pane.active.take() {
                app.source_pane.retire(active);
            }
        } else if !app.source_pane.showing(id) {
            if let Some(active) = app.source_pane.active.take() {
                app.source_pane.retire(active);
            }
            let message = app.session.mailbox.messages.get(&id).ok_or("the message no longer exists")?;
            app.source_pane.active =
                Some(Active { id, opening: app.session.openings(), message: message.clone(), body: Reader::new("mail.sourcePage"), export: None });
        }
        if let Some(active) = &mut app.source_pane.active {
            if let Some(page) = page {
                if active.body.wanted || active.body.error.is_some() {
                    return Err("wait for the current source page".into());
                }
                let offset = if page == "next" {
                    let next = active.body.answer.as_ref().and_then(|v| v["nextOffset"].as_u64()).ok_or("this is the last source page")?;
                    active.body.previous.push(active.body.offset);
                    next
                } else {
                    active.body.previous.pop().ok_or("this is the first source page")?
                };
                active.body.offset = offset;
                active.body.wanted = true;
            }
            if export {
                if active.export.is_some() || app.file_dialog.is_some() {
                    return Err("an export or file dialog is already open".into());
                }
                if app.services.file_dialog.is_none() {
                    return Err("use mail.exportEml for a programmatic export".into());
                }
                active.export = Some(Reader::new("mail.exportEml"));
            }
        }
        Ok(app.source_pane.inspect())
    }
}

/// Whether two versions of a message read as the same source: body, attachments and headers.
fn same_source(a: &Message, b: &Message) -> bool {
    a.id == b.id && a.unfetched == b.unfetched && a.headers == b.headers && a.original == b.original && crate::reading::same_body(a, b)
}

/// Poll actual producer/download results, including cancellation through exit.
pub(crate) fn tick(app: &mut MailApp) {
    let mut pane = std::mem::take(&mut app.source_pane);
    if let Some(mut active) = pane.active.take() {
        let message = app.session.mailbox.messages.get(&active.id).cloned();
        if active.opening != app.session.openings() || app.session.sel.messages.as_slice() != [active.id] || message.is_none() {
            pane.retire(active);
        } else {
            if let Some(message) = message {
                // Flags, read state or sync bookkeeping replace the message but not its
                // source: the reader keeps its token and page.
                if !Arc::ptr_eq(&active.message, &message) && !same_source(&active.message, &message) && !active.body.downloading {
                    if let Some(token) = active.body.token {
                        pane.retiring.push((active.id, active.body.command, token));
                    }
                    active.body = Reader::new("mail.sourcePage");
                }
                active.message = message;
            }
            active.body.step(app, active.id);
            if let Some(export) = &mut active.export {
                export.step(app, active.id);
            }
            pane.active = Some(active);
        }
    }
    pane.retiring.retain(
        |(id, command, token)| matches!(app.session.run(command,&json!({"id":id,"token":token,"close":true})),Ok(value) if value["state"]=="closing"),
    );
    let retiring = &mut pane.retiring;
    let prepared = pane.active.as_mut().and_then(|active| {
        let export = active.export.as_ref()?;
        if let Some(error) = &export.error {
            app.status(error.clone());
            if let Some(token) = export.token {
                retiring.push((active.id, export.command, token));
            }
            active.export = None;
            return None;
        }
        if export.wanted {
            return None;
        }
        let token = export.token?;
        active.export = None;
        Some((active.id, token))
    });
    app.source_pane = pane;
    if let Some((id, token)) = prepared {
        // The picker owns this token from here; cancellation also closes it.
        if let Err(why) = app.run("file.exportMessage", json!({"id":id,"token":token})) {
            let _ = app.session.run("mail.exportEml", &json!({"id":id,"token":token,"close":true}));
            app.status(why);
        }
    }
}

/// A wrapping row within the message header, before any body/remote work.
pub(crate) fn toolbar(app: &mut MailApp, ui: &mut Ui, id: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 12, top: 6, bottom: 0 }).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let on = app.source_pane.showing(id);
            let response =
                ui.selectable_label(on, tl!("Source")).on_hover_text(tl!("Show the HTML or plain-text source without loading external content"));
            app.hit.add("sourceToggle", id, response.rect);
            if response.clicked() {
                let _ = command(app, &json!({"on":!on}));
            }
            if on && ui.button(tl!("Message")).on_hover_text(tl!("Return to the rendered message")).clicked() {
                let _ = command(app, &json!({"on":false}));
            }
        });
    });
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ui, id);
}

pub(crate) fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let mut action = None;
    egui::Frame::NONE.inner_margin(egui::Margin{left:20,right:20,top:8,bottom:12}).show(ui,|ui|{
        let Some(active)=&app.source_pane.active else{return;};
        let reader=&active.body;
        ui.spacing_mut().item_spacing=vec2(6.0,6.0);
        ui.horizontal_wrapped(|ui|{
            let html=reader.answer.as_ref().is_some_and(|answer|answer["format"]=="html");
            ui.strong(if html {tl!("HTML source")}else if reader.answer.is_some(){tl!("Plain-text source")}else{tl!("Message source")});
            let response=ui.add_enabled(active.export.is_none() && app.file_dialog.is_none(),egui::Button::new(tl!("Export .eml")));
            app.hit.add("sourceExport",active.id,response.rect);
            if response.clicked(){action=Some(json!({"export":true}));}
            if active.export.is_some(){ui.spinner();ui.label(tl!("Preparing complete message…"));}
        });
        let retained_original=app.session.mailbox.message(active.id).is_some_and(|message|!message.draft && message.original.is_some());
        let (description,details)=if retained_original {
            (tl!("Text only: nothing runs or downloads. Export preserves the retained protected MIME body."),tl!("The signed or encrypted MIME body is preserved byte for byte; safe message headers are reconstructed. Export fails if the retained original cannot be read completely."))
        } else {
            (tl!("Text only: nothing runs or downloads. Export reconstructs MIME; original bytes are unavailable."),tl!("The retained headers, decoded bodies and attachments are exported completely. Original MIME nesting, encoding and signatures cannot be recreated."))
        };
        ui.add(egui::Label::new(egui::RichText::new(description).color(t.text_dim).size(11.5)).wrap())
            .on_hover_text(details);
        if let Some(error)=&reader.error{ui.colored_label(t.text,error);return;}
        if reader.wanted{ui.horizontal(|ui|{ui.spinner();ui.label(if reader.downloading{tl!("Downloading the complete message…")}else{tl!("Preparing source…")});});return;}
        let Some(answer)=&reader.answer else{return;};
        ui.horizontal_wrapped(|ui|{
            if ui.add_enabled(!reader.previous.is_empty(),egui::Button::new(tl!("Previous page"))).clicked(){action=Some(json!({"page":"previous"}));}
            if ui.add_enabled(answer["nextOffset"].is_u64(),egui::Button::new(tl!("Next page"))).clicked(){action=Some(json!({"page":"next"}));}
            let bytes=answer["bytes"].as_u64().unwrap_or(0);
            let end=answer["nextOffset"].as_u64().unwrap_or(bytes);
            ui.label(egui::RichText::new(format!("{}–{} / {} bytes",reader.offset,end,bytes)).color(t.text_dim).size(11.5));
        });
        let source_label=ui.label(egui::RichText::new(tl!("Selectable message source")).color(t.text_dim).size(11.5));
        app.hit.add("sourceLabel",active.id,source_label.rect);
        let room=ui.available_size().max(vec2(1.0,1.0));
        egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0,t.border)).inner_margin(6.0).show(ui,|ui|{
            let scrolled=egui::ScrollArea::both().id_salt(("source-body",active.id,reader.offset)).max_height((room.y-12.0).max(1.0)).auto_shrink([false,false]).show(ui,|ui|{
                let mut source=answer["source"].as_str().unwrap_or_default();
                // egui37 clamps multiline editors to available width even with
                // desired_width(INFINITY). Give the horizontal scroll content
                // its actual unwrapped width, preserving source line structure.
                // Font layout is cached and only this bounded page is shaped.
                let font=egui::TextStyle::Monospace.resolve(ui.style());
                let width=ui.fonts_mut(|fonts|fonts.layout_no_wrap(source.to_owned(),font,t.text).size().x).max(ui.available_width());
                ui.set_min_width(width);
                let response=ui.add(egui::TextEdit::multiline(&mut source).code_editor().desired_width(width).min_size(vec2(width,0.0)).frame(egui::Frame::NONE)).labelled_by(source_label.id);
                app.hit.add("sourceBody",active.id,response.rect);
            });
            app.hit.add("sourceViewport",active.id,scrolled.inner_rect);
        });
    });
    if let Some(params) = action
        && let Err(why) = command(app, &params)
    {
        app.status(why);
    }
}
