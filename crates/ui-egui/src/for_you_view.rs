// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you ([`mailcraft_engine::for_you`]): the heading of its list above the list of All
//! mail and the Inbox (`mail_view::for_you_panel`), with Show all and the gear, and its settings'
//! page in File.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::for_you::{ForYouFolder, ForYouPrefs};
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// How many For you lists and Show all, beside its title.
const COUNT_TEXT: f32 = 12.5;

/// File › For you's page ([`crate::backstage::PAGES`]).
pub(crate) const PAGE: &str = "forYou";

/// File › For you: the names being typed (kept as typed, commas and all, while the page is in
/// view), the change waiting for a drag or the typing to end (each change recounts the mailbox),
/// and Clear link history asking first.
#[derive(Default)]
pub(crate) struct Settings {
    names: Option<String>,
    draft: Option<ForYouPrefs>,
    confirm_clear: bool,
}

/// For you's title in `gr` (the list header's, in the tabs' `text_size`, v0.12.46): "For you", how
/// many rows it lists, Show all or Show fewer while rows are held back or all of them are shown,
/// and the gear. In a narrow list the count gives way first, then Show all: false when Show all
/// is wanted and found no room here (For you's list then ends with it, [`show_all`]).
pub(crate) fn header(app: &mut MailApp, ui: &mut Ui, gr: Rect, shown: usize, more: bool, text_size: f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter().clone();
    let y = gr.center().y;
    let gear = Rect::from_center_size(pos2(gr.max.x - 16.0, y), vec2(18.0, 18.0));
    let room = (gear.min.x - 8.0 - (gr.min.x + 16.0)).max(0.0);
    let w = paint_clipped(&p, pos2(gr.min.x + 16.0, y), room, tl!("For you"), regular(text_size), t.text);
    let mut x = gr.min.x + 16.0 + w + 8.0;
    let end = gear.min.x - 4.0;
    let link = show_all_label(app, more);
    let width = |s: &str| p.layout_no_wrap(s.to_string(), regular(COUNT_TEXT), t.text).size().x;
    let count = shown.to_string();
    let (count_w, link_w) = (width(&count), link.map_or(0.0, width));
    if x + count_w + link.map_or(0.0, |_| 12.0 + link_w) <= end {
        p.text(pos2(x, y), Align2::LEFT_CENTER, count, regular(COUNT_TEXT), t.text_dim);
        x += count_w + 12.0;
    }
    let placed = match link {
        Some(label) if x + link_w <= end => {
            show_all(app, ui, pos2(x, y), label);
            true
        }
        Some(_) => false,
        None => true,
    };
    let resp = ui
        .interact(gear, ui.scope_id().with("for_you_gear"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tl!("For you settings"));
    app.hit.add("forYouGear", 0, gear);
    if resp.hovered() {
        p.rect_filled(gear, 4.0, t.hover);
    }
    icons::paint(&p, gear.shrink(2.0), "settings", t.icon, t.accent);
    if resp.clicked() {
        let _ = app.run("ui.backstage", json!({"value": true, "page": PAGE}));
    }
    placed
}

/// Show all while For you holds rows back (`more`), Show fewer while it shows them all, or nothing.
pub(crate) fn show_all_label(app: &MailApp, more: bool) -> Option<&'static str> {
    let all = app.session.view.for_you_all;
    (more || all).then(|| if all { tl!("Show fewer") } else { tl!("Show all") })
}

/// Show all or Show fewer, `label`, from `left` (its left middle): a link that turns it over.
pub(crate) fn show_all(app: &mut MailApp, ui: &mut Ui, left: egui::Pos2, label: &str) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter().clone();
    let all = app.session.view.for_you_all;
    let galley = p.layout_no_wrap(label.to_string(), regular(COUNT_TEXT), t.accent_text);
    let at = Rect::from_min_size(pos2(left.x, left.y - galley.size().y / 2.0), galley.size());
    let resp = ui.interact(at.expand(2.0), ui.scope_id().with("for_you_show_all"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    app.hit.add("forYouShowAll", 0, at);
    if resp.hovered() {
        p.line_segment([pos2(at.min.x, at.max.y), at.max], Stroke::new(1.0, t.accent_text));
    }
    p.galley(at.min, galley, t.accent_text);
    if resp.clicked() {
        let _ = app.run("forYou.showAll", json!({"value": !all}));
    }
}

/// File › For you (v0.12.47, a page of File as Options is, where the window of v0.12.43 was):
/// where For you shows, what qualifies, the folders the Inbox's brings back, and the link history.
pub(crate) fn page(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    crate::backstage::heading(ui, tl!("For you"), t);
    ui.label(egui::RichText::new(tl!("Unread mail worth reading first, in a list of its own above the message list.")).color(t.text_dim));
    let before = app.session.prefs.for_you.clone();
    let mut p = app.for_you_settings.draft.take().unwrap_or_else(|| before.clone());
    let mut names = app.for_you_settings.names.take().unwrap_or_else(|| p.own_names.join(", "));
    let mut clear = false;
    let mut confirm = app.for_you_settings.confirm_clear;
    let mut typing = false;
    let mut hits: Vec<(&'static str, Rect)> = Vec::new();

    crate::backstage::section(ui, tl!("Where it shows"), t);
    ui.checkbox(&mut p.all_mail, tl!("In All mail"));
    ui.checkbox(&mut p.inbox, tl!("In the Inbox, with the unread mail your rules file into other folders"));

    crate::backstage::section(ui, tl!("What qualifies"), t);
    crate::backstage::row(ui, tl!("Only mail from the last"), |ui| {
        hits.push(("forYouMaxDays", ui.add(egui::DragValue::new(&mut p.max_days).range(1..=ForYouPrefs::MAX_DAYS)).rect));
        ui.label(egui::RichText::new(tl!("days")).color(t.text_dim));
    });
    tier(ui, t, &mut p.new_sender, tl!("New senders, first seen within"), &mut p.new_sender_days, ForYouPrefs::MAX_DAYS, tl!("days"));
    tier(ui, t, &mut p.replied, tl!("People you reply to, among their latest"), &mut p.replied_last, ForYouPrefs::MAX_LAST, tl!("messages"));
    tier(ui, t, &mut p.links, tl!("Senders whose links you open, over"), &mut p.links_days, ForYouPrefs::MAX_DAYS, tl!("days"));
    tier(ui, t, &mut p.read, tl!("Senders you read, over"), &mut p.read_days, ForYouPrefs::MAX_DAYS, tl!("days"));
    crate::backstage::row(ui, tl!("Show at most"), |ui| {
        hits.push(("forYouCap", ui.add(egui::DragValue::new(&mut p.cap).range(0..=ForYouPrefs::MAX_CAP)).rect));
        ui.label(egui::RichText::new(tl!("messages (0: all); the rest under Show all")).color(t.text_dim));
    });
    crate::backstage::row(ui, tl!("Your names on notifications"), |ui| {
        let resp = ui
            .add(egui::TextEdit::singleline(&mut names).hint_text(tl!("from your addresses")).desired_width(240.0))
            .on_hover_text(tl!("Mail with one of these as its sender's name is your own doing (a notification of your own push), never in For you"));
        if resp.lost_focus() || resp.changed() {
            p.own_names = names.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
        }
        typing = resp.has_focus();
    });

    crate::backstage::section(ui, tl!("Folders in the Inbox's For you"), t);
    ui.label(
        egui::RichText::new(tl!(
            "Unread mail your rules file into these folders comes back to the Inbox's For you: always, or until it is older than a limit."
        ))
        .color(t.text_dim),
    );
    let groups = folders(app);
    if groups.is_empty() {
        ui.label(egui::RichText::new(tl!("No folders mail is filed into.")).color(t.text_dim));
    }
    for (account, folders) in &groups {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(account.as_str()).font(semibold(12.5)).color(t.text));
        egui::Grid::new(("for_you_folders", account.as_str())).num_columns(3).spacing(vec2(16.0, 6.0)).show(ui, |ui| {
            for (id, name) in folders {
                let mut setting = p.folder(*id).cloned().unwrap_or(ForYouFolder { folder: *id, ..Default::default() });
                ui.label(name.as_str());
                ui.checkbox(&mut setting.always, tl!("Always"));
                ui.horizontal(|ui| {
                    let mut limited = setting.max_age_minutes.is_some();
                    ui.checkbox(&mut limited, tl!("Age limit"));
                    let mut minutes = setting.max_age_minutes.unwrap_or(30);
                    if limited {
                        ui.add(egui::DragValue::new(&mut minutes).range(1..=ForYouPrefs::MAX_AGE_MINUTES));
                        ui.label(egui::RichText::new(tl!("minutes")).color(t.text_dim));
                    }
                    setting.max_age_minutes = limited.then_some(minutes);
                });
                ui.end_row();
                p.folders.retain(|f| f.folder != *id);
                if setting.always || setting.max_age_minutes.is_some() {
                    p.folders.push(setting);
                }
            }
        });
    }

    crate::backstage::section(ui, tl!("Link history"), t);
    ui.label(egui::RichText::new(tl!("For you remembers which messages you opened a link in, for Senders whose links you open.")).color(t.text_dim));
    // No undo brings the history back: a second press, Clear, forgets it.
    ui.horizontal(|ui| {
        let ask =
            ui.add_enabled(!confirm, egui::Button::new(tl!("Clear link history"))).on_hover_text(tl!("Forget which messages you opened a link in"));
        hits.push(("forYouClearLinks", ask.rect));
        if ask.clicked() {
            confirm = true;
        }
        if confirm {
            let yes = ui.button(tl!("Clear"));
            let no = ui.button(tl!("Cancel"));
            hits.push(("forYouClearConfirm", yes.rect));
            hits.push(("forYouClearCancel", no.rect));
            clear = yes.clicked();
            if clear || no.clicked() {
                confirm = false;
            }
        }
    });
    for (name, rect) in hits {
        app.hit.add(name, 0, rect);
    }
    // Unchanged unless something was: the folders come back in the order the page lists them. A
    // number being dragged or names being typed wait (each change recounts the mailbox): kept
    // until then, or until the page is left ([`settle`]).
    p.folders.sort_by_key(|f| f.folder);
    let mut was = before;
    was.folders.sort_by_key(|f| f.folder);
    if p != was {
        if ui.ctx().dragged_id().is_some() || typing {
            app.for_you_settings.draft = Some(p);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            let _ = app.run("forYou.settings", serde_json::to_value(&p).unwrap_or_default());
        }
    }
    app.for_you_settings.confirm_clear = confirm;
    if clear {
        let _ = app.run("forYou.clearLinks", json!({}));
    }
    app.for_you_settings.names = Some(names);
}

/// Once a frame: away from File › For you, a change it kept waiting is made, and the names being
/// typed and Clear's question are let go of.
pub(crate) fn settle(app: &mut MailApp) {
    if app.ui.backstage && app.ui.backstage_page == PAGE {
        return;
    }
    app.for_you_settings.names = None;
    app.for_you_settings.confirm_clear = false;
    if let Some(p) = app.for_you_settings.draft.take() {
        let _ = app.run("forYou.settings", serde_json::to_value(&p).unwrap_or_default());
    }
}

/// A tier: on or off, its number and what it counts.
fn tier(ui: &mut Ui, t: &Tokens, on: &mut bool, label: &str, n: &mut u16, max: u16, unit: &str) {
    ui.horizontal(|ui| {
        ui.checkbox(on, label);
        ui.add_enabled(*on, egui::DragValue::new(n).range(1..=max));
        ui.label(egui::RichText::new(unit).color(t.text_dim));
    });
}

/// The folders mail is filed into, by account (its name as the folder pane says it), each by its
/// place in the account's tree: those the Inbox's section may bring back.
fn folders(app: &MailApp) -> Vec<(String, Vec<(mailcraft_model::Id, String)>)> {
    let s = &app.session;
    let mb = &s.mailbox;
    let mut groups: Vec<(String, Vec<(mailcraft_model::Id, String)>)> = Vec::new();
    for a in &mb.accounts {
        let mut folders: Vec<(mailcraft_model::Id, String)> = mb
            .folders
            .iter()
            .filter(|f| f.account == a.id && mailcraft_engine::for_you::filed_folder(s, f.id))
            .map(|f| {
                let mut parts = vec![crate::nav::folder_name(f)];
                let mut up = f.parent.and_then(|p| mb.folder(p));
                // No deeper than a mailbox lets a tree go, however a file tangles it.
                while let Some(parent) = up.filter(|_| parts.len() <= 32) {
                    parts.push(crate::nav::folder_name(parent));
                    up = parent.parent.and_then(|p| mb.folder(p));
                }
                parts.reverse();
                (f.id, parts.join(" › "))
            })
            .collect();
        if folders.is_empty() {
            continue;
        }
        folders.sort_by_key(|x| x.1.to_lowercase());
        groups.push((crate::nav::account_name(a), folders));
    }
    groups
}
