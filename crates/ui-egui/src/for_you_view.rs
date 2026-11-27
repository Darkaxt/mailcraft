// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you ([`mailcraft_engine::for_you`]): the heading of its section at the top of All mail
//! and the Inbox, with Show all and the gear, and the window of its settings.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::for_you::{ForYouFolder, ForYouPrefs};
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// The settings window: open or not, the names being typed (kept as typed, commas and all, until
/// the window closes), the change waiting for a drag or the typing to end (each change recounts
/// the mailbox), and Clear link history asking first.
#[derive(Default)]
pub(crate) struct Settings {
    pub open: bool,
    names: Option<String>,
    draft: Option<ForYouPrefs>,
    confirm_clear: bool,
}

/// The section's heading in `gr` (a group heading's place): "For you", how many rows it lists,
/// Show all or Show fewer while rows are held back or all of them are shown, and the gear.
pub(crate) fn header(app: &mut MailApp, ui: &mut Ui, gr: Rect, shown: usize, more: bool, text_size: f32) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter().clone();
    let y = gr.center().y + 3.0;
    let gear = Rect::from_center_size(pos2(gr.max.x - 16.0, y), vec2(18.0, 18.0));
    let room = (gear.min.x - 8.0 - (gr.min.x + 16.0)).max(0.0);
    let w = paint_clipped(&p, pos2(gr.min.x + 16.0, y), room, tl!("For you"), semibold(text_size), t.group_label);
    let mut x = gr.min.x + 16.0 + w + 8.0;
    let count = p.text(pos2(x, y), Align2::LEFT_CENTER, shown.to_string(), regular(text_size - 1.0), t.text_dim).width();
    x += count + 12.0;
    let all = app.session.view.for_you_all;
    if more || all {
        let label = if all { tl!("Show fewer") } else { tl!("Show all") };
        let galley = p.layout_no_wrap(label.to_string(), regular(text_size - 0.5), t.accent_text);
        let at = Rect::from_min_size(pos2(x, y - galley.size().y / 2.0), galley.size());
        if at.max.x < gear.min.x - 4.0 {
            let resp =
                ui.interact(at.expand(2.0), ui.scope_id().with("for_you_show_all"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            app.hit.add("forYouShowAll", 0, at);
            if resp.hovered() {
                p.line_segment([pos2(at.min.x, at.max.y), at.max], Stroke::new(1.0, t.accent_text));
            }
            p.galley(at.min, galley, t.accent_text);
            if resp.clicked() {
                let _ = app.run("forYou.showAll", json!({"value": !all}));
            }
        }
    }
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
        app.for_you_settings.open = true;
    }
}

/// The For you settings window, while open: each change runs `forYou.settings` at once.
pub(crate) fn settings_window(app: &mut MailApp, ctx: &egui::Context) {
    if !app.for_you_settings.open || app.dialog.is_some() || !app.asking.is_empty() {
        if !app.for_you_settings.open {
            app.for_you_settings.names = None;
            app.for_you_settings.draft = None;
            app.for_you_settings.confirm_clear = false;
        }
        return;
    }
    let t = Tokens::get(ctx);
    let before = app.session.prefs.for_you.clone();
    let mut p = app.for_you_settings.draft.take().unwrap_or_else(|| before.clone());
    let mut names = app.for_you_settings.names.take().unwrap_or_else(|| p.own_names.join(", "));
    let mut open = true;
    let mut clear = false;
    let mut confirm = app.for_you_settings.confirm_clear;
    let mut typing = false;
    let mut hits: Vec<(&'static str, Rect)> = Vec::new();
    let room = ctx.content_rect().size();
    let max = vec2(560.0, 640.0).min((room - vec2(24.0, 24.0)).max(vec2(260.0, 220.0)));
    let folders = folders(app);
    egui::Window::new(tl!("For you"))
        .id(egui::Id::unique("for_you_settings"))
        .open(&mut open)
        .collapsible(false)
        .default_size(vec2(520.0, 560.0).min(max))
        .max_size(max)
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(12.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
            egui::ScrollArea::vertical().id_salt("for_you_settings_scroll").max_height((max.y - 80.0).max(0.0)).show(ui, |ui| {
                ui.label(egui::RichText::new(tl!("Unread mail worth reading first, at the top of the list.")).color(t.text_dim));
                ui.checkbox(&mut p.all_mail, tl!("In All mail"));
                ui.checkbox(&mut p.inbox, tl!("In the Inbox (with unread mail filed into other folders)"));
                ui.separator();
                ui.label(egui::RichText::new(tl!("What qualifies")).font(semibold(12.5)));
                tier(ui, &mut p.new_sender, tl!("New senders, first seen within (days)"), &mut p.new_sender_days, ForYouPrefs::MAX_DAYS);
                tier(ui, &mut p.replied, tl!("People you reply to, among their latest (messages)"), &mut p.replied_last, ForYouPrefs::MAX_LAST);
                tier(ui, &mut p.links, tl!("Senders whose links you open, over (days)"), &mut p.links_days, ForYouPrefs::MAX_DAYS);
                tier(ui, &mut p.read, tl!("Senders you read, over (days)"), &mut p.read_days, ForYouPrefs::MAX_DAYS);
                ui.horizontal(|ui| {
                    ui.label(tl!("Show at most"));
                    hits.push(("forYouCap", ui.add(egui::DragValue::new(&mut p.cap).range(0..=ForYouPrefs::MAX_CAP)).rect));
                    ui.label(egui::RichText::new(tl!("messages (0: all); the rest under Show all")).color(t.text_dim));
                });
                ui.horizontal(|ui| {
                    ui.label(tl!("Your names on notifications"));
                    let resp = ui.add(egui::TextEdit::singleline(&mut names).hint_text(tl!("from your addresses")).desired_width(220.0));
                    if resp.lost_focus() || resp.changed() {
                        p.own_names = names.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
                    }
                    typing = resp.has_focus();
                });
                ui.separator();
                ui.label(egui::RichText::new(tl!("Folders in the Inbox's For you")).font(semibold(12.5)));
                if folders.is_empty() {
                    ui.label(egui::RichText::new(tl!("No folders mail is filed into.")).color(t.text_dim));
                }
                egui::Grid::new("for_you_folders").num_columns(3).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                    for (id, name) in &folders {
                        let mut setting = p.folder(*id).cloned().unwrap_or(ForYouFolder { folder: *id, ..Default::default() });
                        ui.label(name.as_str());
                        ui.checkbox(&mut setting.always, tl!("Always"));
                        ui.horizontal(|ui| {
                            let mut limited = setting.max_age_minutes.is_some();
                            ui.checkbox(&mut limited, tl!("Age limit (minutes)"));
                            let mut minutes = setting.max_age_minutes.unwrap_or(30);
                            if limited {
                                ui.add(egui::DragValue::new(&mut minutes).range(1..=ForYouPrefs::MAX_AGE_MINUTES));
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
                ui.separator();
                // No undo brings the history back: a second press, Clear, forgets it.
                ui.horizontal(|ui| {
                    let ask = ui
                        .add_enabled(!confirm, egui::Button::new(tl!("Clear link history")))
                        .on_hover_text(tl!("Forget which messages you opened a link in"));
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
            });
        });
    for (name, rect) in hits {
        app.hit.add(name, 0, rect);
    }
    // Unchanged unless something was: the folders come back in the order the window lists them.
    // A number being dragged or names being typed wait (each change recounts the mailbox), and are
    // kept when the window closes.
    p.folders.sort_by_key(|f| f.folder);
    let mut was = before.clone();
    was.folders.sort_by_key(|f| f.folder);
    if p != was {
        if open && (ctx.dragged_id().is_some() || typing) {
            app.for_you_settings.draft = Some(p);
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            let _ = app.run("forYou.settings", serde_json::to_value(&p).unwrap_or_default());
        }
    }
    app.for_you_settings.confirm_clear = open && confirm;
    if clear {
        let _ = app.run("forYou.clearLinks", json!({}));
    }
    app.for_you_settings.open = open;
    app.for_you_settings.names = open.then_some(names);
}

/// A tier: on or off, and its number.
fn tier(ui: &mut Ui, on: &mut bool, label: &str, n: &mut u16, max: u16) {
    ui.horizontal(|ui| {
        ui.checkbox(on, label);
        ui.add_enabled(*on, egui::DragValue::new(n).range(1..=max));
    });
}

/// The folders mail is filed into, each with its account's address: those the Inbox's section may
/// bring back.
fn folders(app: &MailApp) -> Vec<(mailcraft_model::Id, String)> {
    let s = &app.session;
    s.mailbox
        .folders
        .iter()
        .filter(|f| mailcraft_engine::for_you::filed_folder(s, f.id))
        .map(|f| {
            let account = s.mailbox.account(f.account).map(|a| if a.email.is_empty() { a.name.clone() } else { a.email.clone() }).unwrap_or_default();
            let name = crate::nav::result_folder_label(&s.mailbox, f);
            (f.id, if account.is_empty() { name } else { format!("{name} · {account}") })
        })
        .collect()
}
