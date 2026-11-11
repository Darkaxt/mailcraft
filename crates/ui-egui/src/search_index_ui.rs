// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Search coverage and evidence use the same read-only commands as MCP. Only
//! bounded pages are retained here; aggregate counts remain engine-owned.

use egui::{Ui, vec2};
use serde_json::{Value, json};

use crate::{MailApp, theme::Tokens};

#[derive(Default)]
pub(crate) struct Pane {
    open: bool,
    focus_on_open: bool,
    opening: u64,
    page: Option<Page>,
    retiring: Vec<Page>,
    error: Option<String>,
}

struct Page {
    command: &'static str,
    params: Value,
    token: Option<u64>,
    answer: Option<Value>,
    state: String,
    previous: Vec<u64>,
    /// What a ready result was last checked against ([`observed`]).
    observed: Option<Observed>,
}

/// Mailbox revision, content-index generation and whether the index is busy:
/// a ready inspection can only go stale when one of these changes.
pub(crate) type Observed = (u64, Option<u64>, bool);

pub(crate) fn observed(app: &MailApp) -> Observed {
    (app.session.rev(), app.session.content_index_status().get("generation").and_then(Value::as_u64), app.session.content_index_busy())
}

impl Page {
    fn new(command: &'static str, params: Value) -> Self {
        Self { command, params, token: None, answer: None, state: "pending".into(), previous: Vec::new(), observed: None }
    }

    fn call(&self, close: bool) -> Value {
        let mut params = self.params.clone();
        if let Some(token) = self.token {
            params["token"] = json!(token)
        }
        if close {
            params["close"] = json!(true)
        }
        params
    }
}

impl Pane {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }
    /// A live engine inspection token is held or still being closed.
    #[cfg(test)]
    pub(crate) fn holds_inspection(&self) -> bool {
        !self.retiring.is_empty() || self.page.as_ref().is_some_and(|page| page.token.is_some() && page.state != "closed")
    }
    /// An inspection is still being worked out or closed. A ready (or failed) result held
    /// open in the pane is only something to look at, and does not keep the app from closing:
    /// the session that owns its token joins its readers when it goes.
    pub(crate) fn inspection_running(&self) -> bool {
        !self.retiring.is_empty()
            || self.page.as_ref().is_some_and(|page| page.token.is_some() && !matches!(page.state.as_str(), "ready" | "failed" | "closed"))
    }
    pub(crate) fn inspect(&self) -> Value {
        json!({"open":self.open,"error":self.error,"page":self.page.as_ref().map(|page|json!({
            "command":page.command,"parameters":page.params,"token":page.token,"state":page.state,
            "result":page.answer,"previousPage":!page.previous.is_empty()})),"retiring":self.retiring.len()})
    }
    fn replace(&mut self, page: Option<Page>) {
        if let Some(old) = self.page.take()
            && old.token.is_some()
        {
            self.retiring.push(old)
        }
        self.page = page;
        self.error = None;
    }
}

pub(crate) fn case_command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let object = params.as_object().ok_or("expected case options")?;
    if object.keys().any(|key| key != "sensitive") {
        return Err("unknown case option".into());
    }
    let current = mailcraft_model::search::Query::parse(&app.session.view.search).case_sensitive();
    let sensitive = match params.get("sensitive") {
        Some(value) => value.as_bool().ok_or("sensitive must be a boolean")?,
        None => !current,
    };
    let query = mailcraft_model::search::Query::with_case(&app.session.view.search, sensitive).map_err(str::to_owned)?;
    app.execute("view.search", json!({"query":query}))?;
    app.search_text = app.session.view.search.clone();
    Ok(json!({"sensitive":sensitive,"query":app.session.view.search}))
}

pub(crate) fn command(app: &mut MailApp, params: &Value) -> Result<Value, String> {
    let object = params.as_object().ok_or("expected search coverage options")?;
    if object.keys().any(|key| !matches!(key.as_str(), "on" | "view" | "id" | "page" | "attachment" | "field" | "section")) {
        return Err("unknown search coverage option".into());
    }
    let on = match params.get("on") {
        Some(value) => value.as_bool().ok_or("on must be a boolean")?,
        None => true,
    };
    let view = params.get("view").map(|value| value.as_str().ok_or("view must be text")).transpose()?;
    if view.is_some_and(|view| !matches!(view, "overview" | "gaps" | "sources" | "details")) {
        return Err("view is overview, gaps, sources or details".into());
    }
    let page = params
        .get("page")
        .map(|value| value.as_str().filter(|value| matches!(*value, "next" | "previous")).ok_or("page is next or previous"))
        .transpose()?;
    if page.is_some() && (!on || view.is_some()) {
        return Err("page an open inspection without changing its view".into());
    }
    if !on {
        app.search_index_pane.replace(None);
        app.search_index_pane.open = false;
        return Ok(app.search_index_pane.inspect());
    }
    if let Some(page) = page {
        let pane = &mut app.search_index_pane;
        let current = pane.page.as_mut().ok_or("open an inspection first")?;
        if current.state != "ready" {
            return Err("wait for this original inspection or refresh its stale result".into());
        }
        let mut next = Page::new(current.command, current.params.clone());
        next.previous = current.previous.clone();
        let offset = if page == "next" {
            let offset = current
                .answer
                .as_ref()
                .and_then(|answer| answer.get("nextOffset"))
                .and_then(Value::as_u64)
                .ok_or("this is the last inspection page")?;
            next.previous.push(current.params.get("offset").and_then(Value::as_u64).unwrap_or(0));
            offset
        } else {
            next.previous.pop().ok_or("this is the first inspection page")?
        };
        next.params["offset"] = json!(offset);
        pane.replace(Some(next));
    } else if let Some(view) = view {
        let next = match view {
            "overview" => None,
            "gaps" => Some(Page::new("search.gaps", json!({"limit":20,"offset":0}))),
            "sources" => {
                let id = match params.get("id") {
                    Some(value) => value.as_u64().filter(|id| *id > 0).ok_or("id must name one message")?,
                    None => app.session.sel.focus().filter(|_| app.session.sel.messages.len() == 1).ok_or("select one message first")?,
                };
                Some(Page::new("search.sources", json!({"id":id,"query":app.session.view.search,"limit":20,"offset":0})))
            }
            "details" => {
                let id = params.get("id").and_then(Value::as_u64).filter(|id| *id > 0).ok_or("id must name one message")?;
                let mut options = json!({"id":id,"section":params.get("section").and_then(Value::as_str).unwrap_or("gaps"),"limit":20,"offset":0});
                let attachment = params.get("attachment");
                let field = params.get("field");
                if attachment.is_some() == field.is_some() {
                    return Err("select one attachment or retained body field".into());
                }
                if let Some(attachment) = attachment {
                    options["attachment"] = json!(attachment.as_u64().ok_or("attachment must be an index")?);
                }
                if let Some(field) = field {
                    options["field"] = json!(field.as_str().ok_or("field must be text")?);
                }
                Some(Page::new("search.coverageDetails", options))
            }
            _ => None,
        };
        app.search_index_pane.replace(next);
    }
    app.search_index_pane.focus_on_open |= !app.search_index_pane.open;
    app.search_index_pane.open = true;
    app.search_index_pane.opening = app.session.openings();
    Ok(app.search_index_pane.inspect())
}

pub(crate) fn tick(app: &mut MailApp) {
    let mut pane = std::mem::take(&mut app.search_index_pane);
    if pane.open && pane.opening != app.session.openings() {
        // The old Session owns and joins its own readers on replacement.
        pane = Pane::default();
    }
    let mut retiring = Vec::new();
    for page in pane.retiring.drain(..) {
        match app.execute(page.command, page.call(true)) {
            Ok(value) if value.get("state").and_then(Value::as_str) == Some("closing") => retiring.push(page),
            Ok(_) => (),
            Err(why) => pane.error = Some(why),
        }
    }
    pane.retiring = retiring;
    // Pending pages are polled for their result; a ready one only when what it
    // was computed against may have changed, never every frame.
    let now = pane.page.as_ref().filter(|page| page.state == "ready").map(|_| observed(app));
    if pane.retiring.is_empty()
        && let Some(page) = &mut pane.page
        && (page.state == "pending" || (page.state == "ready" && now.is_some_and(|now| page.observed != Some(now))))
    {
        match app.execute(page.command, page.call(false)) {
            Err(why) => {
                page.state = "failed".into();
                pane.error = Some(why)
            }
            Ok(value) => {
                page.token = value.get("token").and_then(Value::as_u64);
                page.state = value.get("state").and_then(Value::as_str).unwrap_or("failed").to_owned();
                if page.state == "ready" {
                    page.answer = value.get("result").cloned();
                    page.observed = Some(observed(app));
                }
                if let Some(why) = value.get("error").and_then(Value::as_str) {
                    pane.error = Some(why.into())
                }
            }
        }
    }
    app.search_index_pane = pane;
}

#[cfg(test)]
impl Pane {
    /// An open ready page for `command`, last checked against `observed`.
    pub(crate) fn ready_for_test(command: &'static str, opening: u64, observed: Observed) -> Self {
        let mut page = Page::new(command, json!({}));
        page.state = "ready".into();
        page.token = Some(1);
        page.observed = Some(observed);
        Self { open: true, opening, page: Some(page), ..Default::default() }
    }
    pub(crate) fn page_state(&self) -> Option<&str> {
        self.page.as_ref().map(|page| page.state.as_str())
    }
}

pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    if app.search_index_pane.open
        && app.dialog.is_none()
        && app.asking.is_empty()
        && crate::keys::take_escape_any(ctx, egui::LayerId::new(egui::Order::Middle, egui::Id::unique("search_content_coverage")))
    {
        let _ = command(app, &json!({"on":false}));
    }
}

fn count(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn states(ui: &mut Ui, value: &Value) {
    for (key, label) in [
        ("indexed", tl!("Indexed")),
        ("pending", tl!("Pending")),
        ("pendingAttachments", tl!("Attachments pending")),
        ("missing", tl!("Unavailable")),
        ("partial", tl!("Partial")),
        ("failed", tl!("Failed")),
        ("unsupported", tl!("Unsupported")),
    ] {
        let amount = count(value, key);
        if amount > 0 {
            ui.label(format!("{label}: {amount}"));
        }
    }
}

fn overview(app: &MailApp, ui: &mut Ui, status: &Value) {
    ui.label(tl!("Search covers retained messages on this device. Mail that has not been discovered or downloaded is not assumed to be indexed."));
    if status.get("coverageCurrent").and_then(Value::as_bool) != Some(true) {
        ui.label(tl!("Updating coverage. The last observed counts are shown below."));
    }
    if let Some(accounts) = status.get("coverage").and_then(|value| value.get("accounts")).and_then(Value::as_array) {
        for account in accounts {
            ui.separator();
            let id = count(account, "account");
            let name = app.session.mailbox.accounts.iter().find(|account| account.id == id).map(|account| account.name.as_str()).unwrap_or("");
            ui.strong(name);
            ui.horizontal_wrapped(|ui| {
                ui.label(crate::i18n::fmt(
                    tl!("Messages: {discovered} discovered, {downloaded} downloaded"),
                    &[("discovered", &count(account, "discovered").to_string()), ("downloaded", &count(account, "downloaded").to_string())],
                ));
            });
            ui.horizontal_wrapped(|ui| states(ui, &account["states"]));
            let attachments = &account["attachments"];
            ui.label(crate::i18n::fmt(
                tl!("Attachments: {discovered} discovered, {downloaded} downloaded"),
                &[("discovered", &count(attachments, "discovered").to_string()), ("downloaded", &count(attachments, "downloaded").to_string())],
            ));
            ui.horizontal_wrapped(|ui| states(ui, &attachments["states"]));
        }
    } else {
        ui.label(tl!("Coverage is not available yet."));
    }
    ui.separator();
    ui.label(tl!("Undownloaded messages may contain additional attachments. OCR text is machine recognized; locations and confidence remain available with each source."));
}

fn readable(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}

fn evidence(pane: &mut Pane, ui: &mut Ui) -> Option<Value> {
    let mut action = None;
    let Some(page) = &pane.page else { return None };
    match page.state.as_str() {
        "pending" => {
            ui.spinner();
            ui.label(tl!("Inspecting the original search snapshot…"));
        }
        "stale" => {
            ui.label(tl!("This message changed while it was being inspected. Refresh to inspect its current content."));
        }
        "failed" => {
            ui.label(tl!("Inspection failed. See the error above, then refresh when its cause is resolved."));
        }
        "ready" => {
            if let Some(answer) = &page.answer {
                let rows = answer.get("sources").or_else(|| answer.get("gaps")).or_else(|| answer.get("records")).and_then(Value::as_array);
                if let Some(rows) = rows {
                    if rows.is_empty() {
                        ui.label(tl!("No entries on this page."));
                    }
                    for row in rows {
                        ui.separator();
                        if let Some(id) = row.get("id").and_then(Value::as_u64) {
                            ui.label(crate::i18n::fmt(tl!("Message {id}"), &[("id", &id.to_string())]));
                        }
                        if let Some(source) = row.get("source") {
                            ui.label(readable(source));
                        }
                        if let Some(state) = row.get("state").and_then(Value::as_str) {
                            ui.label(state);
                        }
                        if let Some(gap) = row.get("firstGap").filter(|value| !value.is_null()) {
                            ui.label(readable(gap));
                        }
                        if let (Some(id), Some(source)) = (row.get("id").and_then(Value::as_u64), row.get("source"))
                            && ui.button(tl!("All source details")).clicked()
                        {
                            let mut options = json!({"view":"details","id":id});
                            if let Some(attachment) = source.get("attachment") {
                                options["attachment"] = attachment.clone();
                            } else if let Some(field) = source.get("field") {
                                options["field"] = field.clone();
                            }
                            action = Some(options);
                        }
                        if let Some(location) = row.get("exampleLocation") {
                            ui.label(readable(location));
                        }
                        if let Some(derivation) = row.get("derivation") {
                            ui.label(readable(derivation));
                        }
                        if row.get("source").is_none() {
                            ui.label(readable(row));
                        }
                    }
                }
                ui.label(crate::i18n::fmt(
                    tl!("Total entries: {count}"),
                    &[("count", &answer.get("totalSources").or_else(|| answer.get("total")).and_then(Value::as_u64).unwrap_or(0).to_string())],
                ));
            }
        }
        _ => (),
    }
    action
}

pub(crate) fn show(app: &mut MailApp, ctx: &egui::Context) {
    if !app.search_index_pane.open || app.dialog.is_some() || !app.asking.is_empty() {
        return;
    }
    let mut pane = std::mem::take(&mut app.search_index_pane);
    let status = app.session.content_index_status();
    let t = Tokens::get(ctx);
    let room = ctx.content_rect().size();
    let panel_max = vec2(760.0, 600.0).min((room - vec2(24.0, 24.0)).max(vec2(240.0, 200.0)));
    let mut open = true;
    let mut close_clicked = false;
    let mut action = None;
    let mut retry = false;
    let mut sensitive = mailcraft_model::search::Query::parse(&app.session.view.search).case_sensitive();
    let before = sensitive;
    egui::Window::new(tl!("Search coverage"))
        .id(egui::Id::unique("search_content_coverage"))
        .open(&mut open)
        .collapsible(false)
        .default_size(vec2(760.0, 560.0).min(panel_max))
        .max_size(panel_max)
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(12.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.horizontal_wrapped(|ui| {
                let overview = ui.selectable_label(pane.page.is_none(), tl!("Overview"));
                if pane.focus_on_open {
                    overview.request_focus();
                    pane.focus_on_open = false;
                }
                if overview.clicked() {
                    action = Some(json!({"view":"overview"}));
                }
                if ui.button(tl!("Missing content")).clicked() {
                    action = Some(json!({"view":"gaps"}));
                }
                if ui
                    .add_enabled(
                        app.session.sel.messages.len() == 1 && !app.session.view.search.trim().is_empty(),
                        egui::Button::new(tl!("Where this message matched")),
                    )
                    .clicked()
                {
                    action = Some(json!({"view":"sources"}));
                }
            });
            ui.checkbox(&mut sensitive, tl!("Case sensitive"));
            if let Some(why) = pane.error.as_deref().or_else(|| status.get("problem").and_then(Value::as_str)) {
                ui.colored_label(t.red, why);
            }
            ui.separator();
            egui::ScrollArea::vertical().id_salt("search_coverage_content").min_scrolled_height(0.0).max_height((panel_max.y - 240.0).max(0.0)).show(
                ui,
                |ui| {
                    ui.set_max_width(ui.available_width());
                    if pane.page.is_none() {
                        overview(app, ui, &status)
                    } else if let Some(details) = evidence(&mut pane, ui) {
                        action = Some(details);
                    }
                },
            );
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if let Some(page) = &pane.page {
                    if ui.add_enabled(!page.previous.is_empty() && page.state == "ready", egui::Button::new(tl!("Previous"))).clicked() {
                        action = Some(json!({"page":"previous"}));
                    }
                    let next = page.state == "ready"
                        && page.answer.as_ref().and_then(|answer| answer.get("nextOffset")).is_some_and(|value| !value.is_null());
                    if ui.add_enabled(next, egui::Button::new(tl!("Next"))).clicked() {
                        action = Some(json!({"page":"next"}));
                    }
                    if ui.add_enabled(page.state != "pending", egui::Button::new(tl!("Refresh"))).clicked() {
                        let mut next = Page::new(page.command, page.params.clone());
                        next.previous = page.previous.clone();
                        pane.replace(Some(next));
                    }
                }
                if ui
                    .button(tl!("Retry unavailable content"))
                    .on_hover_text(tl!("Re-read retained missing or failed content. Does not download or change mail."))
                    .clicked()
                {
                    retry = true;
                }
                if ui.button(tl!("Close")).clicked() {
                    close_clicked = true;
                }
            });
        });
    app.search_index_pane = pane;
    if !open || close_clicked {
        action = Some(json!({"on":false}));
    }
    if let Some(params) = action
        && let Err(why) = app.execute("ui.searchIndex", params)
    {
        app.search_index_pane.error = Some(why);
    }
    if sensitive != before
        && let Err(why) = app.execute("ui.searchCase", json!({"sensitive":sensitive}))
    {
        app.search_index_pane.error = Some(why);
    }
    if retry && let Err(why) = app.execute("search.retryIndex", json!({})) {
        app.search_index_pane.error = Some(why);
    }
}
