// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The existing folder tree and a focused conflict review, using real native commands.

use crate::{MailApp, theme::Tokens};
use egui::{RichText, Ui};
use mailcraft_model::Id;
use serde_json::{Value, json};

pub(crate) fn begin(app: &mut MailApp, ids: Vec<Id>, folder: Id) {
    let params = json!({"ids":ids,"folder":folder});
    match app.execute("filing.preview", params.clone()) {
        Ok(report) if report["conflicts"].as_array().is_some_and(Vec::is_empty) => {
            let mut apply = params;
            apply["review"] = report["review"].clone();
            let _ = app.run("filing.apply", apply);
        }
        Ok(_) => {
            let _ = app.run("ui.dialog", json!({"name":"filing","ids":ids,"folder":folder}));
        }
        Err(error) => {
            let _ = app.run("ui.dialog", json!({"name":"filing","ids":ids,"folder":folder,"error":error}));
        }
    }
}

/// The folders in `marked`, each under its account and parent folders, sharing the branches they
/// have in common; each marked one in its colour with a word on what it is.
fn mini_tree(app: &MailApp, ui: &mut Ui, marked: &[(Id, egui::Color32, &str)], t: &Tokens) {
    let mailbox = &app.session.mailbox;
    let mut shown: Vec<(Id, Option<Id>)> = Vec::new(); // (account, folder) already drawn
    for (leaf, colour, what) in marked {
        let Some(folder) = mailbox.folder(*leaf) else { continue };
        let mut chain = vec![*leaf];
        let mut parent = folder.parent;
        while let Some(id) = parent {
            chain.push(id);
            parent = mailbox.folder(id).and_then(|f| f.parent);
        }
        chain.reverse();
        let account = folder.account;
        if !shown.contains(&(account, None)) {
            shown.push((account, None));
            let label =
                mailbox.account(account).map(|a| if a.email.trim().is_empty() { a.name.clone() } else { a.email.clone() }).unwrap_or_default();
            ui.label(RichText::new(label).color(t.text_dim));
        }
        for (depth, id) in chain.iter().enumerate() {
            if shown.contains(&(account, Some(*id))) {
                continue;
            }
            shown.push((account, Some(*id)));
            let is_leaf = id == leaf;
            let name = mailbox.folder(*id).map(|f| crate::nav::folder_name(f).to_string()).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.add_space(16.0 * (depth + 1) as f32);
                let (r, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                crate::icons::paint(ui.painter(), r, "folder", if is_leaf { *colour } else { t.text_dim }, t.accent);
                if is_leaf {
                    ui.label(RichText::new(name).strong().color(*colour));
                    ui.weak(format!("· {what}"));
                } else {
                    ui.label(name);
                }
            });
        }
    }
}

pub struct FilingForm {
    params: Value,
    report: Option<Value>,
    error: String,
    choice: String,
    reuse: Option<Id>,
    unavailable_offset: usize,
    outcome: Option<Value>,
}

impl FilingForm {
    pub fn new(params: &Value) -> Self {
        Self {
            params: json!({"ids":params["ids"],"folder":params["folder"]}),
            report: None,
            error: params["error"].as_str().unwrap_or("").to_owned(),
            choice: "once".into(),
            reuse: None,
            unavailable_offset: 0,
            outcome: None,
        }
    }

    fn refresh(&mut self, app: &mut MailApp) {
        let mut params = self.params.clone();
        params["unavailableOffset"] = json!(self.unavailable_offset);
        match app.execute("filing.preview", params) {
            Ok(report) => {
                self.error.clear();
                let destinations: Vec<_> = report["conflicts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|r| r["destinations"].as_array().into_iter().flatten())
                    .filter_map(|d| d["id"].as_u64())
                    .collect();
                if self.reuse.is_none_or(|id| !destinations.contains(&id)) {
                    self.reuse = destinations.first().copied();
                }
                self.report = Some(report);
            }
            Err(error) => {
                self.error = error;
                self.report = None;
            }
        }
    }

    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        if let Some(outcome) = &self.outcome {
            ui.label(RichText::new(tl!("Some messages couldn't be moved")).strong());
            ui.label(crate::i18n::fmt(
                tl!("{moved} moved, {left} not moved. Those are listed below."),
                &[("moved", &outcome["processed"].to_string()), ("left", &outcome["unavailableTotal"].to_string())],
            ));
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for item in outcome["unavailable"].as_array().into_iter().flatten() {
                    ui.label(format!("#{}: {}", item["id"], item["reason"].as_str().unwrap_or("?")));
                }
            });
            return ui.button(tl!("Close")).clicked();
        }
        if self.report.is_none() && self.error.is_empty() {
            self.refresh(app);
        }
        let t = Tokens::get(ui.ctx());
        let mut page = None;
        let room = (ui.ctx().content_rect().height() - 220.0).max(120.0);
        egui::ScrollArea::vertical().id_salt("filing_review_body").max_height(room).show(ui, |ui| {
            if let Some(report) = &self.report {
                // Plain words, no rules named (owner, 0.12.5: "Too much text. It looks like an FBI
                // warning"; "do not mention the existing rule, lets make that transparent"): where this
                // mail usually goes, both folders in a small tree, and three choices.
                let ids: Vec<Id> = self.params["ids"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
                let sender = ids
                    .first()
                    .and_then(|id| app.session.mailbox.message(*id))
                    .map(|m| if m.from.name.trim().is_empty() { m.from.email.clone() } else { m.from.name.clone() })
                    .unwrap_or_default();
                let mut usual: Vec<Id> = Vec::new();
                for id in report["conflicts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|r| r["destinations"].as_array().into_iter().flatten())
                    .filter_map(|d| d["id"].as_u64())
                {
                    if !usual.contains(&id) {
                        usual.push(id);
                    }
                }
                let chosen = report["destination"]["id"].as_u64().or(self.params["folder"].as_u64());
                let name = |id: Id| app.session.mailbox.folder(id).map(|f| crate::nav::folder_name(f).to_string()).unwrap_or_default();
                let usual_names = usual.iter().map(|id| name(*id)).collect::<Vec<_>>().join(", ");
                let chosen_name = chosen.map(name).unwrap_or_default();
                let headline = if ids.len() == 1 {
                    tl!("This message from {sender} usually goes to {folder}.")
                } else {
                    tl!("Mail from {sender} usually goes to {folder}.")
                };
                ui.label(RichText::new(crate::i18n::fmt(headline, &[("sender", &sender), ("folder", &usual_names)])).strong());
                ui.add_space(6.0);
                let mut marked: Vec<(Id, egui::Color32, &str)> = usual.iter().map(|id| (*id, t.orange, tl!("where it goes now"))).collect();
                if let Some(id) = chosen {
                    marked.push((id, t.green, tl!("the folder you chose")));
                }
                mini_tree(app, ui, &marked, &t);
                ui.add_space(8.0);
                ui.label(tl!("What would you like to do?"));
                ui.radio_value(
                    &mut self.choice,
                    "reuse".into(),
                    if ids.len() == 1 {
                        crate::i18n::fmt(tl!("Move it to {folder} instead"), &[("folder", &usual_names)])
                    } else {
                        crate::i18n::fmt(tl!("Move them to {folder} instead"), &[("folder", &usual_names)])
                    },
                );
                if usual.len() > 1 {
                    ui.indent("reuse_destination", |ui| {
                        ui.add_enabled_ui(self.choice == "reuse", |ui| {
                            egui::ComboBox::from_id_salt("filing_reuse")
                                .width(300.0)
                                .selected_text(self.reuse.map(name).unwrap_or_default())
                                .show_ui(ui, |ui| {
                                    for id in &usual {
                                        ui.selectable_value(&mut self.reuse, Some(*id), name(*id));
                                    }
                                });
                        });
                    });
                }
                ui.add_enabled_ui(report["rerouteAllowed"] == true, |ui| {
                    ui.radio_value(
                        &mut self.choice,
                        "reroute".into(),
                        crate::i18n::fmt(
                            tl!("Move all mail from {sender} in {from} to {to}, now and from now on"),
                            &[("sender", &sender), ("from", &usual_names), ("to", &chosen_name)],
                        ),
                    );
                });
                if report["rerouteAllowed"] != true {
                    ui.indent("reroute_note", |ui| {
                        ui.weak(tl!("Not available: your mail server decides where this mail goes."));
                    });
                }
                ui.radio_value(
                    &mut self.choice,
                    "once".into(),
                    if ids.len() == 1 {
                        crate::i18n::fmt(tl!("Just move this message to {folder}"), &[("folder", &chosen_name)])
                    } else {
                        crate::i18n::fmt(tl!("Just move these messages to {folder}"), &[("folder", &chosen_name)])
                    },
                );
                if let Some(items) = report["unavailable"].as_array().filter(|items| !items.is_empty()) {
                    ui.add_space(4.0);
                    ui.colored_label(
                        t.orange,
                        crate::i18n::fmt(tl!("{count} messages can't be moved right now."), &[("count", &report["unavailableTotal"].to_string())]),
                    );
                    egui::CollapsingHeader::new(tl!("Unavailable messages")).show(ui, |ui| {
                        egui::ScrollArea::vertical().max_height(100.0).show(ui, |ui| {
                            for item in items {
                                ui.label(format!("#{}: {}", item["id"], item["reason"].as_str().unwrap_or("?")));
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.add_enabled(self.unavailable_offset > 0, egui::Button::new(tl!("Previous"))).clicked() {
                                page = Some(self.unavailable_offset.saturating_sub(200));
                            }
                            if let Some(next) = report["nextUnavailableOffset"].as_u64()
                                && ui.button(tl!("Next")).clicked()
                            {
                                page = Some(next as usize);
                            }
                        });
                    });
                }
            }
            if !self.error.is_empty() {
                ui.colored_label(t.red, &self.error);
            }
        });
        ui.separator();
        let mut close = false;
        let mut apply = false;
        let mut refresh = false;
        ui.horizontal(|ui| {
            let enabled = self.report.is_some()
                && (self.choice != "reuse" || self.reuse.is_some())
                && (self.choice != "reroute" || self.report.as_ref().is_some_and(|r| r["rerouteAllowed"] == true));
            let response = ui.add_enabled(enabled, egui::Button::new(tl!("Move")));
            app.hit.add("filingApply", 0, response.rect);
            apply = response.clicked();
            let response = ui.button(tl!("Cancel"));
            app.hit.add("filingCancel", 0, response.rect);
            close = response.clicked();
            // Only after something went wrong: look again at the mailbox as it is now.
            if !self.error.is_empty() {
                let response = ui.button(tl!("Try Again"));
                app.hit.add("filingRefresh", 0, response.rect);
                refresh = response.clicked();
            }
        });
        if let Some(offset) = page {
            self.unavailable_offset = offset;
            refresh = true;
        }
        if refresh {
            self.refresh(app);
        }
        if apply {
            let mut params = self.params.clone();
            params["review"] = self.report.as_ref().map(|r| r["review"].clone()).unwrap_or(Value::Null);
            params["choice"] = json!(self.choice);
            params["reuseFolder"] = json!(self.reuse);
            match app.execute("filing.apply", params) {
                Ok(outcome) => {
                    if outcome["unavailableTotal"].as_u64().unwrap_or(0) == 0 {
                        return true;
                    }
                    self.outcome = Some(outcome);
                    self.report = None;
                }
                Err(error) => {
                    self.error = error;
                    self.report = None;
                }
            }
        }
        close
    }
}

#[cfg(test)]
#[path = "filing_tests.rs"]
mod tests;
