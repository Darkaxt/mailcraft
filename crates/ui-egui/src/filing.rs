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

fn place(value: &Value) -> String {
    format!("{} — {} (#{})", value["path"].as_str().unwrap_or("?"), value["email"].as_str().unwrap_or("?"), value["id"])
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
            ui.label(RichText::new(tl!("Filing changes are queued")).strong());
            ui.label(tl!("The mailbox writer must finish saving, and provider placement still requires confirmation. Do not repeat the filing command; inspect filing status or retry the pending failed item."));
            ui.label(format!("{}: {} · {}: {}", tl!("Processed"), outcome["processed"], tl!("Unavailable"), outcome["unavailableTotal"]));
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for item in outcome["unavailable"].as_array().into_iter().flatten() {
                    ui.label(format!("#{}: {}", item["id"], item["reason"].as_str().unwrap_or("?")));
                }
            });
            ui.label(tl!("All failed history items are retained with their rules-processing status."));
            return ui.button(tl!("Close")).clicked();
        }
        if self.report.is_none() && self.error.is_empty() {
            self.refresh(app);
        }
        let t = Tokens::get(ui.ctx());
        let mut page = None;
        let room = (ui.ctx().content_rect().height() - 220.0).max(120.0);
        egui::ScrollArea::vertical().id_salt("filing_review_body").max_height(room).show(ui,|ui| {
        if let Some(report)=&self.report {
            ui.label(RichText::new(tl!("Existing rules use other destinations")).strong());
            ui.label(format!("{}: {}",tl!("Requested folder"),place(&report["destination"])));
            ui.label(format!("{}: {}",tl!("Exact transport senders"),report["senders"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
            ui.label(tl!("The new filing rule covers all configured accounts. Original-sender display names and forwarded header claims are not substituted for transport addresses."));
            egui::ScrollArea::vertical().max_height(250.0).id_salt("filing_conflicts").show(ui,|ui| {
                for rule in report["conflicts"].as_array().into_iter().flatten() {
                    ui.separator();
                    ui.label(RichText::new(format!("{} (#{})",rule["name"].as_str().unwrap_or("?"),rule["id"])).strong());
                    ui.label(format!("{}: {} · {}: {} · {}: {}",tl!("Account scope"),rule["account"],tl!("Enabled"),rule["enabled"],tl!("Overlap"),rule["certainty"].as_str().unwrap_or("?")));
                    ui.label(if rule["account"]==0 {tl!("All accounts")} else {rule["accountEmail"].as_str().unwrap_or("?")});
                    ui.label(rule["text"].as_str().unwrap_or("?"));
                    for destination in rule["destinations"].as_array().into_iter().flatten() {ui.label(place(destination));}
                    if let Some(reason)=rule["rerouteBlocked"].as_str() {ui.colored_label(t.orange,reason);}
                }
            });
            ui.separator();
            ui.radio_value(&mut self.choice,"reuse".into(),tl!("Use an existing destination; keep the standing rules"));
            ui.indent("reuse_destination",|ui| {
                egui::ComboBox::from_id_salt("filing_reuse").width(570.0)
                    .selected_text(self.reuse.map(|id|app.session.mailbox.folder_path(id)).unwrap_or_default()).show_ui(ui,|ui| {
                        for destination in report["conflicts"].as_array().into_iter().flatten()
                            .flat_map(|r|r["destinations"].as_array().into_iter().flatten()) {
                            if let Some(id)=destination["id"].as_u64() {ui.selectable_value(&mut self.reuse,Some(id),place(destination));}
                        }
                    });
                ui.label(tl!("Disabled rules remain disabled. Only the selected messages move; their one-time placement is preserved."));
            });
            ui.add_enabled_ui(report["rerouteAllowed"]==true,|ui| {
                ui.radio_value(&mut self.choice,"reroute".into(),tl!("Reroute these senders and move their matching received history"));
            });
            ui.indent("historical_scope",|ui| {
                ui.label(format!("{}: {}",tl!("Matching received messages"),report["historicalMessages"]));
                ui.label(tl!("Only these sender alternatives are split. Other senders, rule actions and enabled states remain intact. A new ordinary filing rule covers these senders across accounts; future messages are included. Existing one-time exceptions are excluded from history."));
                egui::ScrollArea::vertical().max_height(105.0).id_salt("filing_history").show(ui,|ui| {
                    for entry in report["historicalScope"].as_array().into_iter().flatten() {
                        ui.label(format!("{}: {}",place(&entry["folder"]),entry["messages"]));
                    }
                });
            });
            ui.radio_value(&mut self.choice,"once".into(),tl!("Move this selection once; leave future sender rules alone"));
            ui.label(tl!("A per-message exception survives reopening and repeated sync. It does not exempt future mail."));
            if let Some(items)=report["unavailable"].as_array().filter(|items|!items.is_empty()) {
                ui.colored_label(t.orange,format!("{}: {}",tl!("Unavailable for relocation"),report["unavailableTotal"]));
                egui::CollapsingHeader::new(tl!("Unavailable messages")).show(ui,|ui| {
                    egui::ScrollArea::vertical().max_height(100.0).show(ui,|ui| {
                        for item in items {ui.label(format!("#{}: {}",item["id"],item["reason"].as_str().unwrap_or("?")));}
                    });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(self.unavailable_offset>0,egui::Button::new(tl!("Previous"))).clicked() {page=Some(self.unavailable_offset.saturating_sub(200));}
                        if let Some(next)=report["nextUnavailableOffset"].as_u64()
                            && ui.button(tl!("Next")).clicked() {
                            page=Some(next as usize);
                        }
                    });
                });
            }
        }
        if !self.error.is_empty() {ui.colored_label(t.red,&self.error);}
        });
        ui.separator();
        let mut close = false;
        let mut apply = false;
        let mut refresh = false;
        ui.horizontal(|ui| {
            let enabled = self.report.is_some()
                && (self.choice != "reuse" || self.reuse.is_some())
                && (self.choice != "reroute" || self.report.as_ref().is_some_and(|r| r["rerouteAllowed"] == true));
            let response = ui.add_enabled(enabled, egui::Button::new(tl!("Apply filing")));
            app.hit.add("filingApply", 0, response.rect);
            apply = response.clicked();
            let response = ui.button(tl!("Refresh review"));
            app.hit.add("filingRefresh", 0, response.rect);
            refresh = response.clicked();
            let response = ui.button(tl!("Cancel"));
            app.hit.add("filingCancel", 0, response.rect);
            close = response.clicked();
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
