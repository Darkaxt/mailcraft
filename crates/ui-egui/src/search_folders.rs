// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Saved-query navigation and editor in the existing folder-pane/form design.

use crate::{
    MailApp,
    theme::{Tokens, paint_clipped, regular, semibold},
};
use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Scope;
use mailcraft_model::{FolderKind, SearchFolder};
use serde_json::{Value, json};

pub(crate) fn navigation(app: &mut MailApp, ui: &mut Ui) {
    egui::CollapsingHeader::new(tl!("Search folders")).default_open(true).show(ui, |ui| {
        let response = ui.button(tl!("New search folder…"));
        app.hit.add("searchFolderNew", 0, response.rect);
        if response.clicked() {
            let _ = app.run("ui.dialog", json!({"name":"searchFolder"}));
        }
        let searches = app.session.mailbox.search_folders.clone();
        if searches.is_empty() {
            ui.label(egui::RichText::new(tl!("Save a search to use it again without moving mail.")).small().weak());
            return;
        }
        let t = Tokens::get(ui.ctx());
        egui::ScrollArea::vertical().id_salt("search_folders").max_height(7.0 * 26.0).show_rows(ui, 26.0, searches.len(), |ui, range| {
            for search in searches.get(range).unwrap_or_default() {
                let problem = mailcraft_engine::cmd::search_folders::anchor_problem(&app.session, search);
                let selected = app.session.view.search_folder == Some(search.id);
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                if selected || response.hovered() {
                    ui.painter().rect_filled(rect.shrink(1.0), 4.0, if selected { t.selection } else { t.hover_nav });
                }
                if response.has_focus() {
                    ui.painter().rect_stroke(rect.shrink(1.0), 4.0, Stroke::new(1.0, t.focus_ring), egui::StrokeKind::Inside);
                }
                crate::icons::paint(
                    ui.painter(),
                    Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(14.0, 14.0)),
                    "search",
                    t.icon,
                    t.accent,
                );
                paint_clipped(
                    ui.painter(),
                    pos2(rect.left() + 24.0, rect.center().y - 7.0),
                    (rect.width() - 28.0).max(0.0),
                    &search.name,
                    if selected { semibold(12.0) } else { regular(12.0) },
                    if problem.is_some() { t.text_disabled } else { t.text },
                );
                let label = match &problem {
                    Some(reason) => format!("{} — {}: {reason}", search.name, tl!("Unavailable")),
                    None => search.name.clone(),
                };
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
                app.hit.add("searchFolder", search.id, rect);
                response.context_menu(|ui| {
                    if ui.button(tl!("Edit search folder…")).clicked() {
                        let _ = app.run("ui.dialog", json!({"name":"searchFolder","id":search.id}));
                        ui.close();
                    }
                    if ui.button(tl!("Delete search folder")).clicked() {
                        let _ = app.run("searchFolders.remove", json!({"id":search.id}));
                        ui.close();
                    }
                    ui.label(tl!("Deletes only this saved search, never its messages."));
                });
                if response.on_hover_text(format!("{label}\n{}", search.query)).clicked() {
                    if problem.is_some() {
                        let _ = app.run("ui.dialog", json!({"name":"searchFolder","id":search.id}));
                    } else {
                        let _ = app.run("searchFolders.select", json!({"id":search.id}));
                    }
                }
            }
        });
    });
    ui.add_space(5.0);
}

pub struct SearchForm {
    definition: SearchFolder,
    original: SearchFolder,
    editing: bool,
    error: String,
    preview: Option<Value>,
    folder_filter: String,
    focus: bool,
}

impl SearchForm {
    pub fn new(app: &MailApp, params: &Value) -> Option<Self> {
        let id = params.get("id").and_then(Value::as_u64);
        if params.get("id").is_some() && id.is_none() {
            return None;
        }
        let definition = match id {
            Some(id) => app.session.mailbox.search_folders.iter().find(|s| s.id == id)?.clone(),
            None => {
                let v = &app.session.view;
                let query = params.get("query").and_then(Value::as_str).unwrap_or(&v.search);
                let scope = match params.get("scope") {
                    Some(value) => serde_json::from_value::<Scope>(value.clone()).ok()?,
                    None => v.scope,
                };
                SearchFolder {
                    name: String::new(),
                    query: if query.trim().is_empty() { "is:unread".into() } else { query.to_owned() },
                    scope,
                    folder: Some(v.folder).filter(|id| app.session.mailbox.folder(*id).is_some()),
                    filing: v.filing,
                    ..Default::default()
                }
            }
        };
        let error = mailcraft_engine::cmd::search_folders::anchor_problem(&app.session, &definition).unwrap_or_default();
        Some(Self {
            original: definition.clone(),
            definition,
            editing: id.is_some(),
            error,
            preview: None,
            folder_filter: String::new(),
            focus: true,
        })
    }

    pub fn title(&self) -> String {
        self.definition.name.clone()
    }
    pub fn dirty(&self) -> bool {
        self.definition != self.original
    }

    fn params(&self) -> Value {
        let mut value = json!({"name":self.definition.name,"query":self.definition.query,"scope":self.definition.scope});
        if self.editing {
            value["id"] = json!(self.definition.id);
        }
        let needs_anchor = matches!(self.definition.scope, Scope::Folder | Scope::Subfolders | Scope::Mailbox) || self.definition.filing.is_some();
        if needs_anchor && let Some(folder) = self.definition.folder {
            value["folder"] = json!(folder);
        }
        if let Some(account) = self.definition.filing {
            value["filing"] = json!(account);
        }
        value
    }

    fn anchor(&mut self, app: &MailApp, ui: &mut Ui) {
        let folder = self.definition.folder.and_then(|id| app.session.mailbox.folder(id));
        let account = folder.map(|f| f.account);
        egui::ComboBox::from_id_salt("search_folder_account")
            .width(470.0)
            .selected_text(account.and_then(|id| app.session.mailbox.account(id)).map(|a| a.email.as_str()).unwrap_or(tl!("Choose account")))
            .show_ui(ui, |ui| {
                for a in app.session.mailbox.accounts_in_order() {
                    if ui.selectable_label(account == Some(a.id), &a.email).clicked() {
                        self.definition.folder = app
                            .session
                            .mailbox
                            .folder_of_kind(a.id, FolderKind::Inbox)
                            .or_else(|| app.session.mailbox.folders.iter().find(|f| f.account == a.id).map(|f| f.id));
                        // Changing an account is explicit editor input. A filing binding is
                        // retained until its checkbox is changed, never guessed from a name.
                    }
                }
            });
        let account = self.definition.folder.and_then(|id| app.session.mailbox.folder(id)).map(|f| f.account);
        let label = self.definition.folder.map(|id| app.session.mailbox.folder_path(id)).unwrap_or_else(|| tl!("Choose real folder").to_owned());
        egui::ComboBox::from_id_salt("search_folder_anchor").width(470.0).selected_text(label).show_ui(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.folder_filter).hint_text(tl!("Filter folder paths")).char_limit(120));
            let wanted = self.folder_filter.to_lowercase();
            let mut folders: Vec<_> = app
                .session
                .mailbox
                .folders
                .iter()
                .filter(|f| Some(f.account) == account)
                .map(|f| (f.id, app.session.mailbox.folder_path(f.id)))
                .filter(|(_, path)| path.to_lowercase().contains(&wanted))
                .collect();
            folders.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
            egui::ScrollArea::vertical().max_height(200.0).show_rows(ui, 24.0, folders.len(), |ui, range| {
                for (id, path) in folders.get(range).unwrap_or_default() {
                    if ui.selectable_label(self.definition.folder == Some(*id), format!("{path} (#{id})")).clicked() {
                        self.definition.folder = Some(*id);
                        ui.close();
                    }
                }
            });
        });
        let selected_account =
            self.definition.folder.and_then(|id| app.session.mailbox.folder(id)).and_then(|f| app.session.mailbox.account(f.account));
        let can_file = selected_account.is_some_and(|a| a.kind == mailcraft_model::AccountKind::Microsoft && !a.is_shared());
        let mut filing = self.definition.filing.is_some();
        let response = ui.add_enabled(can_file || filing, egui::Checkbox::new(&mut filing, tl!("Use the unified filing tree")));
        if response.changed() {
            self.definition.filing = if filing { selected_account.map(|a| a.id) } else { None };
        }
    }

    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let previous = self.definition.clone();
        let room = (ui.ctx().content_rect().height() - 220.0).max(150.0);
        egui::ScrollArea::vertical().id_salt("search_folder_editor").max_height(room).show(ui, |ui| {
            ui.label(tl!("A search folder is a saved view. It never relocates or duplicates messages."));
            ui.label(tl!("Name"));
            let response =
                ui.add(egui::TextEdit::singleline(&mut self.definition.name).char_limit(mailcraft_model::MAX_NAME).desired_width(f32::INFINITY));
            app.hit.add("searchFolderName", 0, response.rect);
            if std::mem::take(&mut self.focus) {
                response.request_focus();
            }
            ui.label(tl!("Query"));
            let response = ui.add(
                egui::TextEdit::multiline(&mut self.definition.query)
                    .char_limit(mailcraft_model::search::text::MAX_QUERY_BYTES)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            app.hit.add("searchFolderQuery", 0, response.rect);
            ui.label(tl!(
                "Use double quotes for an exact phrase and case:sensitive for letter-case matching. alias: matches the attributed receiving alias."
            ));
            ui.label(tl!("Search scope"));
            egui::ComboBox::from_id_salt("saved_query_scope")
                .width(470.0)
                .selected_text(crate::chrome::scopes().iter().find(|(s, ..)| *s == self.definition.scope).map_or("?", |(_, name, _)| *name))
                .show_ui(ui, |ui| {
                    for (scope, name, _) in crate::chrome::scopes() {
                        ui.selectable_value(&mut self.definition.scope, scope, name);
                    }
                });
            if matches!(self.definition.scope, Scope::Folder | Scope::Subfolders | Scope::Mailbox) || self.definition.filing.is_some() {
                self.anchor(app, ui);
            }
            if let Some(why) = mailcraft_engine::cmd::search_folders::anchor_problem(&app.session, &self.definition) {
                ui.colored_label(t.red, why);
            }
            if let Some(why) = mailcraft_model::search::Query::parse(&self.definition.query).error() {
                ui.colored_label(t.red, why);
            }
            if !self.error.is_empty() {
                ui.colored_label(t.red, &self.error);
            }
            if let Some(preview) = &self.preview {
                ui.separator();
                ui.label(format!("{}: {}", tl!("Known matching messages"), preview["total"]));
                ui.label(tl!("This preview uses locally downloaded/indexed content. Coverage gaps are not evidence that a message does not exist."));
                if preview["searchIndex"]["complete"] != true {
                    ui.colored_label(
                        t.orange,
                        tl!(
                            "Index coverage is incomplete or unavailable. These matches may omit mail; open the results to inspect coverage details."
                        ),
                    );
                }
                for m in preview["messages"].as_array().into_iter().flatten() {
                    ui.label(format!("#{} · {} · {}", m["id"], m["subject"].as_str().unwrap_or(""), m["from"].as_str().unwrap_or("")));
                }
                if !preview["nextOffset"].is_null() {
                    ui.label(tl!("First 50 matches shown. Open the search folder for all results and coverage details."));
                }
            }
        });
        if self.definition != previous {
            self.preview = None;
            self.error.clear();
        }
        ui.separator();
        let mut save = false;
        let mut preview = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let valid = !self.definition.name.trim().is_empty()
                && !self.definition.query.trim().is_empty()
                && mailcraft_model::search::Query::parse(&self.definition.query).error().is_none()
                && mailcraft_engine::cmd::search_folders::anchor_problem(&app.session, &self.definition).is_none();
            let response = ui.add_enabled(valid, egui::Button::new(tl!("Save search folder")));
            app.hit.add("searchFolderSave", 0, response.rect);
            save = response.clicked();
            let response = ui.add_enabled(valid, egui::Button::new(tl!("Preview")));
            app.hit.add("searchFolderPreview", 0, response.rect);
            preview = response.clicked();
            let response = ui.button(tl!("Cancel"));
            app.hit.add("searchFolderCancel", 0, response.rect);
            cancel = response.clicked();
        });
        if preview {
            match app.execute("searchFolders.preview", self.params()) {
                Ok(value) => {
                    self.preview = Some(value);
                    self.error.clear();
                }
                Err(error) => self.error = error,
            }
        }
        if save {
            match app.execute(if self.editing { "searchFolders.edit" } else { "folder.newSearchFolder" }, self.params()) {
                Ok(_) => return true,
                Err(error) => self.error = error,
            }
        }
        cancel
    }
}

#[cfg(test)]
#[path = "search_folders_tests.rs"]
mod tests;
