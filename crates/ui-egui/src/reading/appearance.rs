// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Display-only message colours and a read-only preview of the current draft.

use std::sync::Arc;

use egui::Ui;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Place;
use crate::{MailApp, theme::Tokens};

/// A message's presentation, independent from the surrounding window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageTheme {
    #[default]
    App,
    Light,
    Dark,
}

impl MessageTheme {
    fn code(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    fn dark(self, app_dark: bool) -> bool {
        match self {
            Self::App => app_dark,
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

fn choice(app: &MailApp, place: Place) -> MessageTheme {
    match place {
        Place::Pane | Place::Window(_) => app.ui.message_theme.unwrap_or(if app.ui.light_messages { MessageTheme::Light } else { MessageTheme::App }),
        Place::DraftPreview(_) => app.ui.draft_preview_theme,
        _ => MessageTheme::App,
    }
}

pub(super) fn dark(app: &MailApp, ctx: &egui::Context, place: Place) -> bool {
    choice(app, place).dark(Tokens::get(ctx).dark)
}

pub(crate) fn command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let object = params.as_object().ok_or("expected message display options")?;
    if object.keys().any(|key| !matches!(key.as_str(), "target" | "value")) {
        return Err("unknown message display option".into());
    }
    let theme = match params.get("value").and_then(Value::as_str) {
        Some("app") => MessageTheme::App,
        Some("light") => MessageTheme::Light,
        Some("dark") => MessageTheme::Dark,
        _ => return Err("value must be app, light or dark".into()),
    };
    let target = match params.get("target") {
        None => "reader",
        Some(value) => value.as_str().ok_or("target must be reader or draft")?,
    };
    match target {
        "reader" => {
            app.ui.message_theme = Some(theme);
            // Keep old clients/settings meaningful without letting them override this choice.
            app.ui.light_messages = theme == MessageTheme::Light;
        }
        "draft" => app.ui.draft_preview_theme = theme,
        _ => return Err("target must be reader or draft".into()),
    }
    Ok(json!({"target":target,"value":theme.code()}))
}

pub(crate) fn preview_command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let object = params.as_object().ok_or("expected draft preview options")?;
    if object.keys().any(|key| key != "on") {
        return Err("unknown draft preview option".into());
    }
    let id = app.session.compose.as_ref().ok_or("no message is being written")?.id;
    let binding = (app.session.openings(), id);
    let on = match params.get("on") {
        Some(value) => value.as_bool().ok_or("on must be a boolean")?,
        None => app.ui.preview_compose != Some(binding),
    };
    app.ui.preview_compose = on.then_some(binding);
    Ok(json!({"compose":id,"on":on}))
}

/// The three choices of message appearance, as selectable labels: in the reader's floating
/// menu ([`super::controls`]) and in the draft preview's row.
pub(super) fn controls(app: &mut MailApp, ui: &mut Ui, place: Place) {
    let selected = choice(app, place);
    let (target, id) = match place {
        Place::DraftPreview(id) => ("draft", id),
        Place::Window(id) => ("reader", id),
        _ => ("reader", 0),
    };
    for (value, label, hit) in [
        (MessageTheme::Light, tl!("Light"), "messageThemeLight"),
        (MessageTheme::Dark, tl!("Dark"), "messageThemeDark"),
        (MessageTheme::App, tl!("App theme"), "messageThemeApp"),
    ] {
        let response = ui.selectable_label(selected == value, label);
        app.hit.add(hit, id, response.rect);
        if response.clicked() {
            let _ = app.run("ui.messageTheme", json!({"target":target,"value":value.code()}));
        }
    }
}

/// Both embedded and separate compose windows use this before editable fields.
/// The native message builder is shared with Save/Send; neither action is run.
pub(crate) fn compose_preview(app: &mut MailApp, ui: &mut Ui) -> bool {
    let Some(id) = app.session.compose.as_ref().map(|compose| compose.id) else {
        ui.ctx().data_mut(|data| data.remove::<Arc<CachedPreview>>(preview_key()));
        return false;
    };
    let mut showing = app.ui.preview_compose == Some((app.session.openings(), id));
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 12, top: 4, bottom: 4 }).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(8.0, 4.0);
            let label = if showing { tl!("Back to editing") } else { tl!("Preview message") };
            let response = ui.button(label);
            app.hit.add("draftPreview", id, response.rect);
            if response.clicked() && preview_command(app, &json!({"on":!showing})).is_ok() {
                showing = !showing;
            }
            if showing {
                ui.label(tl!("Read-only preview"));
                controls(app, ui, Place::DraftPreview(id));
            }
        });
    });
    if !showing {
        ui.ctx().data_mut(|data| {
            if data.get_temp::<Arc<CachedPreview>>(preview_key()).is_some_and(|cached| cached.compose.id == id) {
                data.remove::<Arc<CachedPreview>>(preview_key());
            }
        });
        return false;
    }
    let message = match preview_message(app, ui.ctx()) {
        Ok(message) => message,
        Err(error) => {
            ui.label(format!("{error}"));
            return true;
        }
    };
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 4, bottom: 6 }).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(&message.subject).strong()).wrap());
        ui.add(egui::Label::new(format!("{}: {}", tl!("From"), mailcraft_model::format_list(std::slice::from_ref(&message.from)))).wrap());
        ui.add(egui::Label::new(format!("{}: {}", tl!("To"), mailcraft_model::format_list(&message.to))).wrap());
        if !message.cc.is_empty() {
            ui.add(egui::Label::new(format!("Cc: {}", mailcraft_model::format_list(&message.cc))).wrap());
        }
    });
    let place = Place::DraftPreview(id);
    super::view::page(app, place, ui).show(ui, |ui| {
        crate::mail_view::steady_scroll_bar(app, ui);
        egui::ScrollArea::both().auto_shrink([false, false]).id_salt(("draft-preview", id)).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(16).show(ui, |ui| {
                super::show_plain_view(app, ui, &message, place);
            });
        });
    });
    true
}

struct CachedPreview {
    compose: mailcraft_engine::Compose,
    opening: u64,
    revision: u64,
    message: Arc<mailcraft_model::Message>,
}

fn preview_key() -> egui::Id {
    egui::Id::unique("mailcraft-draft-preview-message")
}

/// Retain one derived preview, not a full freshly assembled HTML body each frame.
fn preview_message(app: &MailApp, ctx: &egui::Context) -> Result<Arc<mailcraft_model::Message>, mailcraft_engine::CmdError> {
    let compose = app.session.compose.as_ref().ok_or_else(|| mailcraft_engine::CmdError::Failed("no message is being written".into()))?;
    if let Some(cached) = ctx.data(|data| data.get_temp::<Arc<CachedPreview>>(preview_key()))
        && cached.opening == app.session.openings()
        && cached.compose == *compose
        && cached.revision == app.session.rev()
    {
        return Ok(cached.message.clone());
    }
    let message = Arc::new(mailcraft_engine::cmd::compose::preview(&app.session)?);
    let cached =
        Arc::new(CachedPreview { compose: compose.clone(), opening: app.session.openings(), revision: app.session.rev(), message: message.clone() });
    ctx.data_mut(|data| data.insert_temp(preview_key(), cached));
    Ok(message)
}

pub(super) fn retire_closed_preview(app: &MailApp, ctx: &egui::Context) {
    ctx.data_mut(|data| {
        if data
            .get_temp::<Arc<CachedPreview>>(preview_key())
            .is_some_and(|cached| cached.opening != app.session.openings() || app.session.composing_by_id(cached.compose.id).is_none())
        {
            data.remove::<Arc<CachedPreview>>(preview_key());
        }
    });
}
