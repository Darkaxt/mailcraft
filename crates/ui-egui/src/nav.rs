// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Getting around: the switch between the modules (Mail, Calendar, People, Tasks), as a bar of
//! icons down the left edge or as a row at the foot of the pane on the left (#1174), and the
//! folder pane beside it in Mail.

use std::collections::{HashMap, HashSet};

use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Module;
use mailcraft_engine::cmd::folder::sort_as_listed;
use mailcraft_model::{Account, Folder, FolderKind, GmailCategory, Id, Mailbox};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::favorites_sync::Note;
use crate::theme::{Tokens, bold, paint_clipped, regular};
use crate::{MailApp, icons};

/// The module bar's width, and the height each module takes in it.
const RAIL_W: f32 = 48.0;
const RAIL_PITCH: f32 = 48.0;

/// A folder's row.
const ROW_H: f32 = 24.0;
/// Folder names and counts are 9 pt, the Favorites and account headings 11 pt.
const NAME_SIZE: f32 = 12.0;
const HEADING_SIZE: f32 = 14.667;
/// Where a top-level name starts, and how much further in each level under it starts. This and
/// the distances below are from the pane's edges, the line down its left edge included.
const NAME_LEFT: f32 = 26.0;
const INDENT: f32 = 9.0;
/// The deepest level that still moves further in (deeper folders line up with it).
const MAX_INDENT_DEPTH: usize = 16;
/// Where a row's or a heading's chevron is centred.
const CHEVRON_X: f32 = 15.0;
/// Half the width of a heading's chevron, and of a row's.
const HEADING_CHEVRON: f32 = 4.75;
const ROW_CHEVRON: f32 = 4.25;
/// Counts end this far from the pane's right edge.
const COUNT_RIGHT: f32 = 16.0;
/// The highlight of a row (selected, or under the pointer) stops this short of the pane's edges.
const HIGHLIGHT_LEFT: f32 = 6.0;
const HIGHLIGHT_RIGHT: f32 = 4.0;
/// Inset each row's background so adjacent highlights have a gap without shrinking the hit area.
const ROW_HIGHLIGHT_INSET: f32 = 1.0;
/// A line of text is centred by its line box; the eye centres it by its lower-case letters, which
/// sit this much below the middle of the box.
/// How round the pane's corner is where it meets the module bar and the ribbon.
const SHEET_CORNER: u8 = 8;
/// How wide the strip is that stays when the pane is hidden.
const STRIP_W: f32 = 30.0;

/// The id the Favorites heading is folded under in `UiState::collapsed_folders`, beside the
/// folders' ids and the accounts' (an account's heading folds under the account's own id).
/// Accounts and folders are numbered from 1 in one sequence, so none of the three collide.
const FAVORITES: Id = 0;
/// The id the Recent heading folds under (v0.12.14), one no account or folder reaches.
const RECENT: Id = Id::MAX;
/// The id the All Accounts heading (#625) is folded under, past any account's or folder's (and
/// beside Recent's).
pub(crate) const ALL_ACCOUNTS: Id = Id::MAX - 1;
/// The room an account's origin-colour swatch takes after its name in its heading (#893).
const ACCOUNT_DOT_ROOM: f32 = 16.0;

/// The module bar. It runs from under the title bar down to the status bar, beside the tabs and
/// the ribbon as well as the panes, so the window lays it out before the ribbon.
pub fn module_bar(app: &mut MailApp, ui: &mut Ui) {
    // At the foot of the pane on the left instead, where older mail programs had it (#1174).
    if foot_in_use(&app.session) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    egui::Panel::left("modules").exact_size(RAIL_W).resizable(false).show_separator_line(false).frame(egui::Frame::NONE.fill(t.nav)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        for (n, (m, icon, id)) in module_buttons().into_iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(RAIL_W, RAIL_PITCH), Sense::click());
            app.hit.add("module", n as u64, r);
            let active = app.session.view.module == m;
            // Messages dragged over Calendar, Tasks or People are taken there (#873); Mail takes nothing.
            let takes = dropped_as(m).is_some();
            let dragged_over = takes && resp.dnd_hover_payload::<crate::mail_view::MessageDrag>().is_some();
            if dragged_over {
                crate::drag_pointer::note_folder(ui.ctx(), true);
                app.hit.add("moduleDrop", n as u64, r);
                ui.painter().rect_filled(r.shrink(6.0), 4.0, t.hover);
                ui.painter().rect_stroke(r.shrink(6.0), 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            }
            if active {
                // The module in view: a bar at the window's edge, and its icon in the accent.
                let bar = Rect::from_min_size(pos2(r.min.x + 6.0, r.center().y - 16.0), vec2(2.0, 32.0));
                ui.painter().rect_filled(bar, 1.0, t.accent);
            } else if resp.hovered() && !dragged_over {
                ui.painter().rect_filled(r.shrink(6.0), 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
            }
            // One colour: the icons of the bar are line drawings, lit when theirs is the module in view.
            let ink = if active { t.accent_text } else { t.icon };
            icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(24.0, 24.0)), icon, ink, ink);
            let sc = crate::widgets::shortcut_text(app, id);
            if takes && let Some(drag) = resp.dnd_release_payload::<crate::mail_view::MessageDrag>() {
                drop_on_module(app, m, &drag.0);
            }
            if resp.on_hover_text(format!("{} ({sc})", tl!(m.label()))).clicked() {
                let _ = app.run(id, json!({}));
            }
        }
        // What a .pst file's notes and journal entries will be read in: kept, with no view yet.
        for (icon, name) in [("notes", tl!("Notes")), ("journal", tl!("Journal"))] {
            let (r, resp) = ui.allocate_exact_size(vec2(RAIL_W, RAIL_PITCH), Sense::hover());
            let ink = t.icon.linear_multiply(0.45);
            icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(24.0, 24.0)), icon, ink, ink);
            resp.on_hover_text(format!("{name}: {}", tl!("Pending development")));
        }
    });
}

/// What a message let go over the module `m` becomes (#873): the kind `mail.itemFromMessage`
/// fills in, and the form that opens with it. `None` for Mail.
fn dropped_as(m: Module) -> Option<(&'static str, &'static str)> {
    match m {
        Module::Calendar => Some(("appointment", "event")),
        Module::Tasks => Some(("task", "task")),
        Module::People => Some(("contact", "contact")),
        Module::Mail => None,
    }
}

pub(crate) use mailcraft_engine::sync_driver::fetch::DROP;

/// Messages let go over Calendar, Tasks or People in the module bar (#873), as in classic Outlook:
/// a new appointment (in a window of its own), task or contact opens, filled in from the first of
/// them that is still there: its subject, its text, its sender. Nothing is made until the owner
/// saves it there, and the message stays where it is, as it was. One form opens however many were
/// dragged (a task or a contact is filled in in the one dialog), and the status bar says so. A
/// message of which only the server's short preview is here waits to be downloaded first.
pub(crate) fn drop_on_module(app: &mut MailApp, m: Module, dragged: &[Id]) {
    let name = match m {
        Module::Calendar => "calendar",
        Module::Tasks => "tasks",
        Module::People => "people",
        Module::Mail => return,
    };
    let _ = app.run(DROP, json!({"module": name, "ids": dragged}));
}

/// The first two messages of a drop (`ids`) that are still in the mailbox.
pub(crate) fn dropped(app: &MailApp, p: &serde_json::Value) -> Vec<Id> {
    mailcraft_engine::sync_driver::fetch::dropped(&app.session, p)
}

/// `ui.dropOnModule`; `None` for any other command.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &serde_json::Value) -> Option<Result<serde_json::Value, String>> {
    (id == DROP).then(|| drop_on(app, p))
}

fn drop_on(app: &mut MailApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let (kind, form) = match p.get("module").and_then(serde_json::Value::as_str) {
        Some("calendar") => ("appointment", "event"),
        Some("tasks") => ("task", "task"),
        Some("people") => ("contact", "contact"),
        _ => return Err("`module` is \"calendar\", \"tasks\" or \"people\"".into()),
    };
    let here = dropped(app, p);
    let Some(first) = here.first().copied() else { return Err(tl!("None of those messages is in this mailbox any more.").to_string()) };
    let fields = app.session.run("mail.itemFromMessage", &json!({"id": first, "as": kind})).map_err(|e| {
        app.last_error = Some(e.clone());
        e.to_string()
    })?;
    if form == "event" {
        // (A window that can't open says why in the status bar itself.)
        if let Some(Err(e)) = crate::windows::open_form(app, form, &fields) {
            return Err(e);
        }
    } else {
        match crate::dialogs::open_dialog(app, form, &fields) {
            Some(dialog) => {
                app.set_dialog(Some(dialog));
                app.sync_panel = None;
            }
            None => return Ok(json!({"opened": null})),
        }
    }
    if here.len() > 1 {
        let n = p.get("ids").and_then(serde_json::Value::as_array).map_or(here.len(), Vec::len);
        app.status(tln!(n, "{n} message was dragged: only the first was opened.", "{n} messages were dragged: only the first was opened."));
    }
    Ok(json!({"opened": form, "message": first}))
}

/// How tall the row of modules is at the foot of the pane on the left (#1174), and how large a
/// module's name is written there.
const FOOT_H: f32 = 34.0;
const FOOT_NAME: f32 = 12.0;
/// What a name needs beside its icon in the row: the icon, the gap to it, and a little air.
const FOOT_NAME_ROOM: f32 = 22.0;

/// The icon and command id of each module, in the order both the bar and the foot show them.
fn module_buttons() -> [(Module, &'static str, &'static str); 4] {
    [
        (Module::Mail, "mail", "view.mail"),
        (Module::Calendar, "calendar", "view.calendar"),
        (Module::People, "people", "view.people"),
        (Module::Tasks, "tasks", "view.tasks"),
    ]
}

/// Whether the row of modules sits at the foot of the pane on the left (#1174): the owner asked
/// for it, and the module in view has a pane for it. Mail's folder pane and the calendar's side
/// pane can be hidden, and what is left of Mail's is a strip too narrow for four modules; People
/// and Tasks always have theirs. Without a pane the bar down the window's edge stays, so the
/// switch is never out of reach.
pub fn foot_in_use(s: &mailcraft_engine::Session) -> bool {
    s.view.nav_foot && (s.view.folder_pane || matches!(s.view.module, Module::People | Module::Tasks))
}

/// Whether a module's name is written beside its icon in the row (#1174): all four or none, so
/// that the row doesn't read half named, and only where the pane is wide enough for the longest
/// of them. A narrow pane leaves the icons alone, which is still the whole switch: what the row
/// is for is the column the bar took down the window, not the names.
pub(crate) fn names_fit(each: f32, widest: f32) -> bool {
    each >= widest + FOOT_NAME_ROOM
}

/// The switch between the modules at the foot of the pane on the left (#1174), where older mail
/// programs had it: an icon for each, side by side, with their names where the pane is wide
/// enough ([`names_fit`]), and the module in view in the accent with a line under it. Nothing
/// while the bar down the window's edge is the one in use, so every pane may call it.
pub fn module_foot(app: &mut MailApp, ui: &mut Ui) {
    if !foot_in_use(&app.session) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("modules_foot").exact_size(FOOT_H).resizable(false).show_separator_line(false).frame(egui::Frame::NONE.fill(t.nav)).show(
        ui,
        |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let room = ui.max_rect();
            let each = (room.width() / 4.0).max(1.0);
            // The names show for all four or for none, so the row doesn't read half named as the
            // pane narrows: the longest of them is what has to fit.
            let widest = module_buttons()
                .into_iter()
                .map(|(m, _, _)| crate::theme::text_width(ui.painter(), tl!(m.label()), &regular(FOOT_NAME), 0.0, t.text))
                .fold(0.0f32, f32::max);
            let named = names_fit(each, widest);
            for (i, (m, icon, id)) in module_buttons().into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(room.min.x + each * i as f32, room.min.y), vec2(each, room.height()));
                let resp = ui.interact(r, ui.scope_id().with(("module_foot", icon)), Sense::click());
                let active = app.session.view.module == m;
                if active {
                    let line = Rect::from_min_size(pos2(r.min.x + 4.0, r.max.y - 2.0), vec2((r.width() - 8.0).max(0.0), 2.0));
                    ui.painter().rect_filled(line, 1.0, t.accent);
                } else if resp.hovered() {
                    ui.painter().rect_filled(r.shrink(3.0), 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
                }
                // Reached with Tab or the arrow keys (Enter or Space switches): where the keyboard is.
                if resp.has_focus() {
                    ui.painter().rect_stroke(r.shrink(3.0), 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                let ink = if active { t.accent_text } else { t.icon };
                let name = tl!(m.label());
                let text_w = crate::theme::text_width(ui.painter(), name, &regular(FOOT_NAME), 0.0, ink);
                let icon_x = if named { r.center().x - (text_w + 20.0) / 2.0 + 9.0 } else { r.center().x };
                icons::paint(ui.painter(), Rect::from_center_size(pos2(icon_x, r.center().y), vec2(18.0, 18.0)), icon, ink, ink);
                if named {
                    let at = pos2(icon_x + 11.0, r.center().y);
                    ui.painter().text(at, Align2::LEFT_CENTER, name, regular(FOOT_NAME), if active { t.accent_text } else { t.text });
                }
                app.hit.add("moduleFoot", i as u64, r);
                // For a screen reader: a button, and which module is in view. The module is
                // chosen (AccessKit's `selected`), not switched on, as the Gmail tabs are.
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                ui.ctx().accesskit_node_builder(resp.id, |node| node.set_selected(active));
                let sc = crate::widgets::shortcut_text(app, id);
                if resp.on_hover_text(format!("{name} ({sc})")).clicked() {
                    let _ = app.run(id, json!({}));
                }
            }
        },
    );
}

/// How deep the pane follows subfolders, and how many rows it lists for one account: a mailbox
/// may come from anywhere, and the pane is laid out again every frame. As many rows as a mailbox
/// holds folders (#991: an IMAP account alone may have 10,000), so that none is left out; only
/// the rows in sight are drawn.
const MAX_DEPTH: usize = 32;
const MAX_ROWS: usize = mailcraft_model::MAX_FOLDERS;

/// The folders of `account` as the pane lists them, depth first, skipping what is under a folded
/// one: each with its depth and whether it has subfolders.
#[cfg(test)]
fn visible(mailbox: &Mailbox, folded: &[Id], account: Id) -> Vec<(Folder, usize, bool)> {
    let mut trees = trees(mailbox, |a| a == account);
    visible_in(trees.remove(&account).unwrap_or_default(), folded)
}

/// The folders of each account that `wanted` names, by account and parent, in one pass over all the
/// folders however many accounts there are, each list in the order the pane shows it.
fn trees(mailbox: &Mailbox, wanted: impl Fn(Id) -> bool) -> HashMap<Id, Tree<'_>> {
    let mut trees: HashMap<Id, Tree> = HashMap::new();
    for f in mailbox.folders.iter().filter(|f| wanted(f.account)) {
        trees.entry(f.account).or_default().entry(f.parent).or_default().push(f);
    }
    for tree in trees.values_mut() {
        for (parent, list) in tree.iter_mut() {
            sort_as_listed(list, parent.is_none());
        }
    }
    trees
}

type Tree<'a> = HashMap<Option<Id>, Vec<&'a Folder>>;

fn visible_in(tree: Tree<'_>, folded: &[Id]) -> Vec<(Folder, usize, bool)> {
    let mut out = Vec::new();
    list_under(&tree, folded, None, 0, &mut out);
    out
}

fn list_under(tree: &HashMap<Option<Id>, Vec<&Folder>>, folded: &[Id], parent: Option<Id>, depth: usize, out: &mut Vec<(Folder, usize, bool)>) {
    if depth > MAX_DEPTH {
        return;
    }
    for f in tree.get(&parent).into_iter().flatten() {
        if out.len() >= MAX_ROWS {
            return;
        }
        let has_children = tree.get(&Some(f.id)).is_some_and(|under| !under.is_empty());
        out.push(((*f).clone(), depth, has_children));
        if has_children && !folded.contains(&f.id) {
            list_under(tree, folded, Some(f.id), depth + 1, out);
        }
    }
}

/// What a folder shows beside its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Count {
    Nothing,
    /// Unread messages: the number, bold.
    Unread(usize),
    /// Everything in the folder, for the folders where all of it waits for something (Drafts,
    /// Outbox) or is on its way out (Junk Email): the number in square brackets.
    Total(usize),
}

impl Count {
    /// A folder of `kind` holding `total` messages, `unread` of them unread.
    pub(crate) fn of(kind: FolderKind, total: usize, unread: usize) -> Count {
        match kind {
            FolderKind::Drafts | FolderKind::Outbox | FolderKind::Junk if total > 0 => Count::Total(total),
            FolderKind::Inbox | FolderKind::Archive | FolderKind::Custom if unread > 0 => Count::Unread(unread),
            // Sent and deleted mail asks for nobody's attention, read or not.
            _ => Count::Nothing,
        }
    }

    /// The number alone, as the pill shows it.
    fn digits(self) -> String {
        match self {
            Count::Nothing => String::new(),
            Count::Unread(n) | Count::Total(n) => n.to_string(),
        }
    }

    #[cfg(test)]
    fn text(self) -> String {
        match self {
            Count::Nothing => String::new(),
            Count::Unread(n) => n.to_string(),
            Count::Total(n) => format!("[{n}]"),
        }
    }
}

/// A chevron centred on `c`, `half` its half-width when it points down (what is under it shows);
/// it points right when that is folded away.
fn chevron(painter: &Painter, c: Pos2, half: f32, open: bool, color: Color32) {
    let points = if open {
        vec![pos2(c.x - half, c.y - half / 2.0), pos2(c.x, c.y + half / 2.0), pos2(c.x + half, c.y - half / 2.0)]
    } else {
        vec![pos2(c.x - half / 2.0, c.y - half), pos2(c.x + half / 2.0, c.y), pos2(c.x - half / 2.0, c.y + half)]
    };
    painter.add(Shape::line(points, Stroke::new(1.5, color)));
}

/// Where a row's highlight goes.
fn highlight(r: Rect) -> Rect {
    Rect::from_min_max(pos2(r.min.x + HIGHLIGHT_LEFT, r.min.y), pos2(r.max.x - HIGHLIGHT_RIGHT, r.max.y))
}

/// A heading of the pane (Favorites, an account): a chevron and the name, `height` tall with the
/// name's line centred `text_y` under the top, and a rule above it when `ruled`. A click folds or
/// unfolds what is under it; `true` when that shows. A `movable` one (an account of the owner's
/// own, beside another, #702) is dragged up or down the pane by it.
fn heading(app: &mut MailApp, ui: &mut Ui, id: Id, name: &str, height: f32, text_y: f32, ruled: bool, movable: bool) -> bool {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("heading", id)), if movable { Sense::click_and_drag() } else { Sense::click() });
    app.hit.add("folderHeading", id, r);
    if movable {
        resp.dnd_set_drag_payload(AccountDrag(id));
    }
    if ruled {
        let y = r.min.y + 9.5;
        ui.painter().line_segment([pos2(r.min.x + HIGHLIGHT_LEFT, y), pos2(r.max.x - HIGHLIGHT_RIGHT, y)], Stroke::new(1.0, t.border));
    }
    let open = !app.ui.collapsed_folders.contains(&id);
    let y = r.min.y + text_y;
    chevron(ui.painter(), pos2(r.min.x + CHEVRON_X, y), HEADING_CHEVRON, open, if resp.hovered() { t.accent_text } else { t.text });
    // An account's origin colour (#893), as its rows in combined views wear it: a small swatch
    // after its name, which leaves room for it. (Ringed, so a colour near the pane's shows; in
    // High Contrast the ring is in the text's colour.)
    let color = app.session.mailbox.account(id).map(|a| a.effective_row_color()).filter(|_| id != FAVORITES && id != ALL_ACCOUNTS);
    let room = if color.is_some() { ACCOUNT_DOT_ROOM } else { 0.0 };
    let w =
        paint_clipped(ui.painter(), pos2(r.min.x + NAME_LEFT, y), (r.width() - NAME_LEFT - 8.0 - room).max(0.0), name, regular(HEADING_SIZE), t.text);
    if let Some(color) = color {
        let center = pos2(r.min.x + NAME_LEFT + w + ACCOUNT_DOT_ROOM / 2.0 + 1.0, y + 1.0);
        let at = crate::unified_inbox::swatch(ui.painter(), &t, center, color);
        app.hit.add("accountColor", id, at);
    }
    if resp.clicked() {
        let _ = app.execute("ui.toggleFolder", json!({"id": id}));
    }
    // An account's heading (not the Favorites', Recent's nor All Accounts').
    if id != FAVORITES && id != RECENT && id != ALL_ACCOUNTS {
        resp.context_menu(|ui| {
            ui.set_min_width(190.0);
            account_menu(app, ui, id, movable);
        });
    }
    open
}

/// An account's menu, on its heading or its tab: Move Up and Move Down for a `movable` one,
/// Properties, and its colour.
fn account_menu(app: &mut MailApp, ui: &mut Ui, id: Id, movable: bool) {
    // The keyboard's and the menu's way of a drag (#702), for the accounts that drag.
    if movable {
        let own = owners(&app.session.mailbox);
        for (label, up) in [(tl!("Move Up"), true), (tl!("Move Down"), false)] {
            let before = move_before(&own, id, up);
            if ui.add_enabled(before.is_some(), egui::Button::new(label).min_size(vec2(180.0, 0.0))).clicked() {
                if let Some(before) = before {
                    let _ = app.run("account.move", json!({"id": id, "before": before}));
                }
                ui.close();
            }
        }
        ui.separator();
    }
    if ui.add(egui::Button::new(tl!("Properties…")).min_size(vec2(180.0, 0.0))).clicked() {
        let _ = app.run("folder.properties", json!({"account": id}));
        ui.close();
    }
    crate::unified_inbox::color_menu(app, ui, id);
}

/// An account being dragged up or down the folder pane by its heading (#702).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AccountDrag(pub Id);

/// A favourite kept here being dragged up or down Favorites (v0.12.17).
#[derive(Clone, Copy, Debug)]
pub(crate) struct FavoriteDrag(pub Id);

/// What a favourite's star takes after its name in its account's tree.
const STAR_ROOM: f32 = 18.0;

/// The owner's own accounts, top to bottom: what a drag puts in another order. A shared mailbox
/// goes with the account it is opened through.
fn owners(mb: &Mailbox) -> Vec<Id> {
    mb.accounts_in_order().into_iter().filter(|a| !a.is_shared()).map(|a| a.id).collect()
}

/// What `account.move` is told to put `account` one place up (`up`) or down among the owner's
/// own accounts `own`: the account it goes before, `None` inside for the end. `None` at the top
/// for Move Up, at the bottom for Move Down, and for an account that isn't among them.
fn move_before(own: &[Id], account: Id, up: bool) -> Option<Option<Id>> {
    let at = own.iter().position(|a| *a == account)?;
    if up {
        own.get(at.checked_sub(1)?).map(|a| Some(*a))
    } else {
        own.get(at.checked_add(1)?)?;
        Some(own.get(at.saturating_add(2)).copied())
    }
}

/// A folder being dragged up or down among the folders beside it, or (`favorite`) among the
/// Favorites kept in MailCraft (#1514).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FolderDrag {
    pub id: Id,
    pub favorite: bool,
}

/// What a folder's row moves among (#1514), top to bottom, it among them: under Favorites
/// (`favorite`) the favourites kept in MailCraft, in its account's list the folders beside it
/// but Inbox, Drafts, Sent Items and Deleted Items, which lead. Empty where it doesn't move.
fn reorder_peers(app: &MailApp, id: Id, favorite: bool) -> Vec<Id> {
    if favorite {
        let own = mailcraft_engine::cmd::folder::own_favorites(&app.session);
        if own.contains(&id) { own } else { Vec::new() }
    } else {
        app.session.mailbox.reorder_siblings(id)
    }
}

/// What a folder's row moves before when `dragged` is let go over the row of `target` among
/// `peers` (#1514), in its upper half (`upper`) or its lower: `target` itself, or the one after
/// it (`None` inside: last). `None` when either isn't among them, or they are one.
fn drop_before(peers: &[Id], dragged: Id, target: Id, upper: bool) -> Option<Option<Id>> {
    if dragged == target || !peers.contains(&dragged) {
        return None;
    }
    let rest: Vec<Id> = peers.iter().copied().filter(|p| *p != dragged).collect();
    let at = rest.iter().position(|p| *p == target)?;
    Some(if upper { Some(target) } else { rest.get(at.saturating_add(1)).copied() })
}

/// How far the folder pane scrolls this frame while an account is dragged with the pointer at
/// height `y` (#702): towards the top within a row of the top of the part in view, `pane`,
/// towards the bottom within a row of its bottom, faster the nearer the edge; else nothing. A
/// positive amount moves the rows down (shows what is above).
fn edge_scroll(pane: Rect, y: f32) -> f32 {
    const MOST: f32 = 12.0;
    let (top, bottom) = (y - pane.min.y, pane.max.y - y);
    if top < ROW_H {
        MOST * (1.0 - top.max(0.0) / ROW_H)
    } else if bottom < ROW_H {
        -MOST * (1.0 - bottom.max(0.0) / ROW_H)
    } else {
        0.0
    }
}

/// Where an account of the owner's own starts in the pane, for a drag of one (#702): its
/// heading's top, and the height a line there is drawn at (on the rule above the heading).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Block {
    account: Id,
    top: f32,
    line: f32,
}

/// The gap between the owner's accounts that a pointer at height `y` points at: 0 before the
/// first, `blocks.len()` after the last. Over an account (its heading, its folders, its shared
/// mailboxes) the gap above it in its upper half and the one below in its lower half; above the
/// first (over Favorites) the first gap, below the last the last. `end` is where the last ends.
fn gap_at(blocks: &[Block], end: f32, y: f32) -> usize {
    for (n, block) in blocks.iter().enumerate() {
        let bottom = blocks.get(n + 1).map_or(end, |next| next.top);
        if y < block.top {
            return n;
        }
        if y < bottom {
            return if y < (block.top + bottom) / 2.0 { n } else { n + 1 };
        }
    }
    blocks.len()
}

/// While an account is dragged by its heading (#702): over the pane, a line in the gap between
/// two accounts where it would go, and on letting go there the move (`account.move`). Letting go
/// where it already is changes nothing, and so do Escape and letting go outside the pane.
fn account_drop(app: &mut MailApp, ui: &mut Ui, blocks: &[Block], end: f32) {
    let Some(drag) = egui::DragAndDrop::payload::<AccountDrag>(ui.ctx()) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let Some(from) = blocks.iter().position(|b| b.account == drag.0) else { return };
    // (The part of the pane in view: the scroll area's.) The pointer is over the pane itself, not
    // over a window or an agent's question on top of it.
    let pane = ui.clip_rect();
    if !ui.rect_contains_pointer(pane) {
        return;
    }
    crate::drag_pointer::note_folder(ui.ctx(), true);
    // Near the top or the bottom of what is in view the pane scrolls, to reach an account out of view.
    let step = edge_scroll(pane, pos.y);
    if step != 0.0 {
        ui.scroll_with_delta(vec2(0.0, step));
        ui.ctx().request_repaint();
    }
    let gap = gap_at(blocks, end, pos.y);
    let t = Tokens::get(ui.ctx());
    let y = blocks.get(gap).map_or(end + 2.0, |b| b.line);
    let (left, right) = (pane.min.x + HIGHLIGHT_LEFT, pane.max.x - HIGHLIGHT_RIGHT);
    ui.painter().line_segment([pos2(left + 3.0, y), pos2(right, y)], Stroke::new(2.0, t.accent));
    ui.painter().circle_stroke(pos2(left + 3.0, y), 3.0, Stroke::new(2.0, t.accent));
    app.hit.add("accountGap", gap as u64, Rect::from_min_max(pos2(left, y - 3.0), pos2(right, y + 3.0)));
    if ui.input(|i| i.pointer.any_released()) {
        egui::DragAndDrop::clear_payload(ui.ctx());
        if gap != from && gap != from + 1 {
            let before = blocks.get(gap).map(|b| b.account);
            let _ = app.run("account.move", json!({"id": drag.0, "before": before}));
        }
    }
}

/// While an account or a folder (#1514) is being dragged: a small label by the pointer with its name.
fn drag_ghost(app: &MailApp, ui: &Ui) {
    let name = if let Some(drag) = egui::DragAndDrop::payload::<AccountDrag>(ui.ctx()) {
        let Some(a) = app.session.mailbox.account(drag.0) else { return };
        if a.name.trim().is_empty() { tl!("Mailbox").to_string() } else { shown_name(a).to_string() }
    } else if let Some(drag) = egui::DragAndDrop::payload::<FolderDrag>(ui.ctx()) {
        let Some(f) = app.session.mailbox.folder(drag.id) else { return };
        folder_name(f).to_string()
    } else {
        return;
    };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let t = Tokens::get(ui.ctx());
    egui::Area::new(egui::Id::unique("account_drag_ghost")).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 16.0)).interactable(false).show(
        ui.ctx(),
        |ui| {
            egui::Frame::NONE
                .fill(t.menu)
                .stroke(Stroke::new(1.0, t.border_strong))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(name).font(regular(12.0)).color(t.text));
                });
        },
    );
}

/// Where the folder in view was last picked: `true` in its account's list, `false` under
/// Favorites (and before any pick). A folder listed in both is highlighted in that one place.
fn picked_in_list(ui: &Ui) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::unique("folder_picked_in_list"))).unwrap_or(false)
}

/// A folder's row, `selected` when it is the one the folder in view is highlighted in. `key` tells
/// the Favorites' rows (`fav`) from the accounts' (`tree`) and from the unified mailbox's filing
/// tree (`unified`), whose rows open the filing view and file what is dropped on them.
/// The side of a folder's site icon in its row, in points.
const FOLDER_ICON: f32 = 14.0;

fn folder_row(app: &mut MailApp, ui: &mut Ui, f: &Folder, depth: usize, has_children: bool, key: &str, count: Count, selected: bool) {
    let unified = key == "unified";
    // An account whose Favorites are Outlook's has them changed in Outlook: nothing to do here.
    let from_outlook = mailcraft_engine::cmd::folder::favorites_from_server(&app.session, f.account);
    // A favourite kept here is dragged up or down Favorites (v0.12.17); its children are not.
    let draggable = key == "fav" && depth == 0 && !from_outlook;
    // In its account's tree and in the unified mailbox's a favourite wears a small star after its
    // name (v0.12.17; the unified tree's since v0.12.21), which takes it out of Favorites when
    // clicked (v0.12.24).
    let starred = (key == "tree" || unified)
        && if from_outlook { app.session.mailbox.server_favorites(f.account).is_some_and(|list| list.contains(&f.id)) } else { f.favorite };
    // Under Favorites a shared mailbox's folder says whose it is (#70), and so does one whose name
    // another account's folder has too (two Inboxes); in its tree, its name.
    let name =
        if (key == "fav" && depth == 0) || key == "recent" { result_folder_label(&app.session.mailbox, f) } else { folder_name(f).to_string() };
    let t = Tokens::get(ui.ctx());
    // Moved up or down among the folders beside it in its account's tree, or among the Favorites
    // kept here, with its menu (#1514). In the tree it is dragged so too; a favourite is dragged
    // the Favorites' own way (`draggable`, v0.12.17).
    let favorite_row = key == "fav";
    let movable = if favorite_row { draggable && f.favorite } else { key == "tree" && !mailcraft_model::leads(f) };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with((key, f.id)), if draggable || movable { Sense::click_and_drag() } else { Sense::click() });
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    // (A favourite's folder under Favorites is told from its row in its account's tree.)
    app.hit.add(
        if unified {
            "unifiedFolder"
        } else if key == "fav" && depth > 0 {
            "favoriteFolder"
        } else {
            "folder"
        },
        f.id,
        r,
    );
    // (A favourite carries its own payload, set further down.)
    if movable && !draggable {
        resp.dnd_set_drag_payload(FolderDrag { id: f.id, favorite: favorite_row });
    }
    let row = highlight(r).shrink2(vec2(0.0, ROW_HIGHLIGHT_INSET));
    if selected {
        ui.painter().rect_filled(row, 6.0, t.accent_soft);
        ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 1.5, row.center().y), vec2(3.0, 14.0)), 1.5, t.selection_edge);
    } else if resp.hovered() {
        // Quieter than on the ribbon: the pane is darker, and the selected row must stay the one that stands out.
        ui.painter().rect_filled(row, 6.0, t.hover_nav);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(row, 6.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
    }
    let x = r.min.x + depth.min(MAX_INDENT_DEPTH) as f32 * INDENT;
    // The chevron has its own click target, only on a folder with something under it.
    if has_children {
        let tr = Rect::from_min_max(pos2(x + HIGHLIGHT_LEFT, r.min.y), pos2(x + NAME_LEFT, r.max.y));
        let tresp = ui.interact(tr, ui.scope_id().with((key, "twisty", f.id)), Sense::click());
        // A favourite's own folders under Favorites are folded until opened (v0.12.17).
        let top_favorite = key == "fav" && depth == 0;
        let open = if top_favorite {
            app.ui.open_favorites.contains(&f.id)
        } else {
            !(if unified { &app.ui.collapsed_unified } else { &app.ui.collapsed_folders }).contains(&f.id)
        };
        chevron(ui.painter(), pos2(x + CHEVRON_X, r.center().y), ROW_CHEVRON, open, if tresp.hovered() { t.accent_text } else { t.text });
        if tresp.clicked() {
            let _ = app.execute("ui.toggleFolder", json!({"id": f.id, "unified": unified, "favorites": top_favorite}));
        }
    }
    let y = r.center().y;
    let assigned = app.session.mailbox.rules.iter().any(|rule| rule.targets(f.id));
    let right_edge = r.max.x - if assigned { 24.0 } else { 0.0 };
    let mut rules_clicked = false;
    if assigned {
        let cog = Rect::from_center_size(pos2(r.max.x - 12.0, r.center().y), vec2(22.0, 22.0));
        let button = ui.put(cog, egui::Button::new("").frame(false));
        let account = app.session.mailbox.account(f.account).map(|a| if a.email.is_empty() { shown_name(a) } else { a.email.as_str() }).unwrap_or("");
        let folder = format!("{} - {account}", folder_path(&app.session.mailbox, f.id));
        let label = crate::i18n::fmt(tl!("Rules for {folder}"), &[("folder", &folder)]);
        button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        icons::paint(ui.painter(), cog.shrink(4.0), "settings", if button.hovered() { t.accent_text } else { t.icon }, t.accent);
        if button.has_focus() {
            ui.painter().rect_stroke(cog, 3.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
        }
        app.hit.add("folderRules", f.id, cog);
        rules_clicked = button.on_hover_text(label).clicked();
        if rules_clicked {
            let _ = app.run("ui.dialog", json!({"name": "rules", "destination": f.id}));
        }
    }
    // The count, flush right, in a pill: an unread count in the accent's soft colour, a total
    // (Drafts, Junk) in a quiet grey one.
    let name_end = count_pill(ui.painter(), &t, r, right_edge, count, selected);
    let font = if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) };
    // Before its name, the icon of the site its mail comes from (0.12.34); in its place, while
    // there is none (mixed mail, a mail provider, an icon not found or still coming), its tag
    // (`folder.tag`). The icon wins over the tag (0.12.35): the owner tags nearly every folder.
    let mut name_x = x + NAME_LEFT;
    let icon = if name_end - name_x > FOLDER_ICON + 5.0 { crate::sender_images::folder_icon(app, ui.ctx(), f.id) } else { None };
    if let Some(icon) = icon {
        let at = Rect::from_center_size(pos2(name_x + FOLDER_ICON / 2.0, y), vec2(FOLDER_ICON, FOLDER_ICON));
        // On the pane itself, as a tag's emoji is (v0.12.38: no white tile behind it).
        egui::Image::from_texture(egui::load::SizedTexture::from_handle(&icon.texture)).tint(icon.tint(t.text)).corner_radius(2.0).paint_at(ui, at);
        app.hit.add("folderIcon", f.id, at);
        name_x += FOLDER_ICON + 6.0;
    } else if !f.tag.is_empty() {
        let w = paint_clipped(ui.painter(), pos2(name_x, y), (name_end - name_x).max(0.0), &f.tag, regular(NAME_SIZE), t.text);
        app.hit.add("folderTag", f.id, Rect::from_min_max(pos2(name_x, r.min.y), pos2(name_x + w, r.max.y)));
        name_x += w + 5.0;
    }
    let room = if starred { STAR_ROOM } else { 0.0 };
    let w = paint_clipped(
        ui.painter(),
        pos2(name_x, y),
        (name_end - name_x - room).max(0.0),
        &name,
        font,
        if selected { t.accent_soft_text } else { t.text },
    );
    if starred {
        let star = Rect::from_center_size(pos2(name_x + w + STAR_ROOM / 2.0, y), vec2(12.0, 12.0));
        let hit = ui.interact(star.expand(4.0), ui.scope_id().with((key, "star", f.id)), Sense::click());
        let hint = if from_outlook { tl!("Favorites Are Set in Outlook") } else { tl!("Remove from Favorites") };
        hit.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, !from_outlook, hint));
        icons::paint(ui.painter(), star, "starOn", if hit.hovered() && !from_outlook { t.accent_text } else { t.icon }, t.accent);
        app.hit.add("folderStar", f.id, star);
        if hit.on_hover_text(hint).clicked() && !from_outlook {
            let _ = app.run("folder.favorite", json!({"id": f.id, "value": false}));
        }
    }
    if resp.clicked() && !rules_clicked {
        ui.data_mut(|d| d.insert_temp(egui::Id::unique("folder_picked_in_list"), key == "tree"));
        if unified {
            let _ = app.run("view.filing", json!({"account":f.account,"folder":f.id}));
        } else {
            let _ = app.run("folder.select", json!({"id": f.id}));
        }
    }
    // Messages dragged out of the list are filed in the folder they are let go over.
    if let Some(drag) = resp.dnd_hover_payload::<crate::mail_view::MessageDrag>() {
        let takes = !crate::drag_pointer::movable(&app.session.mailbox, &drag.0, f.id).is_empty();
        crate::drag_pointer::note_folder(ui.ctx(), takes);
        if takes {
            ui.painter().rect_stroke(row, 0.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }
    }
    if let Some(drag) = crate::drag_pointer::released::<crate::mail_view::MessageDrag>(&resp) {
        // Only what isn't here already.
        let ids = crate::drag_pointer::movable(&app.session.mailbox, &drag.0, f.id);
        if !ids.is_empty() {
            if unified {
                crate::filing::begin(app, ids, f.id);
            } else {
                let _ = app.run("mail.move", json!({"ids": ids, "folder": f.id}));
            }
        }
    }
    // A favourite dragged onto another goes before it, or after it from the lower half.
    if draggable {
        resp.dnd_set_drag_payload(FavoriteDrag(f.id));
        let lower = ui.ctx().pointer_hover_pos().is_some_and(|p| p.y > r.center().y);
        if resp.dnd_hover_payload::<FavoriteDrag>().is_some_and(|drag| drag.0 != f.id) {
            let at = if lower { r.max.y } else { r.min.y };
            ui.painter().line_segment([pos2(row.min.x, at), pos2(row.max.x, at)], Stroke::new(2.0, t.accent));
        }
        if let Some(drag) = crate::drag_pointer::released::<FavoriteDrag>(&resp)
            && drag.0 != f.id
        {
            let mut params = json!({"id": drag.0});
            params[if lower { "after" } else { "before" }] = json!(f.id);
            let _ = app.run("folder.favoriteMove", params);
        }
    }
    // A folder dragged over another beside it: a line where it would go, and the move on letting go.
    // (Not on the lower half of one whose subfolders show: the drop would land after them all, a
    // line at this row's edge would lie, so it is no place to drop.)
    let subtree_shows = !favorite_row && has_children && !app.ui.collapsed_folders.contains(&f.id);
    if let Some(drag) = resp.dnd_hover_payload::<FolderDrag>()
        && drag.favorite == favorite_row
        && let Some(pos) = ui.ctx().pointer_interact_pos()
    {
        let upper = pos.y < r.center().y;
        if (upper || !subtree_shows) && drop_before(&reorder_peers(app, f.id, favorite_row), drag.id, f.id, upper).is_some() {
            crate::drag_pointer::note_folder(ui.ctx(), true);
            let y = if upper { r.min.y + 0.5 } else { r.max.y - 0.5 };
            let (left, right) = (r.min.x + depth.min(MAX_INDENT_DEPTH) as f32 * INDENT + HIGHLIGHT_LEFT, r.max.x - HIGHLIGHT_RIGHT);
            ui.painter().line_segment([pos2(left + 3.0, y), pos2(right, y)], Stroke::new(2.0, t.accent));
            ui.painter().circle_stroke(pos2(left + 3.0, y), 3.0, Stroke::new(2.0, t.accent));
            app.hit.add("folderGap", f.id, Rect::from_min_max(pos2(left, y - 3.0), pos2(right, y + 3.0)));
        }
    }
    if egui::DragAndDrop::has_payload_of_type::<FolderDrag>(ui.ctx())
        && let Some(drag) = resp.dnd_release_payload::<FolderDrag>()
        && drag.favorite == favorite_row
        && let Some(pos) = ui.ctx().pointer_interact_pos()
        && (pos.y < r.center().y || !subtree_shows)
        && let Some(before) = drop_before(&reorder_peers(app, f.id, favorite_row), drag.id, f.id, pos.y < r.center().y)
    {
        let cmd = if favorite_row { "folder.reorderFavorite" } else { "folder.reorder" };
        let _ = app.run(cmd, json!({"id": drag.id, "before": before}));
    }
    // (One of the server's own, Conversation History say, is left as it is, and takes no new
    // folder either: #756.)
    let (id, custom, favorite, kind) = (f.id, f.kind == FolderKind::Custom && !f.server_owned(), f.favorite, f.kind);
    let takes_folders = !(f.kind == FolderKind::Custom && f.server_owned());
    let tag = f.tag.clone();
    let recent_row = key == "recent";
    resp.context_menu(|ui| {
        ui.set_min_width(190.0);
        let item = |ui: &mut Ui, app: &mut MailApp, label: &str, cmd: &str, params: serde_json::Value, enabled: bool| {
            if ui.add_enabled(enabled, egui::Button::new(tl!(label)).min_size(vec2(180.0, 0.0))).clicked() {
                let _ = app.run(cmd, params);
                ui.close();
            }
        };
        // The keyboard's and the menu's way of a drag (#1514), where the row moves.
        if movable {
            let peers = reorder_peers(app, id, favorite_row);
            let cmd = if favorite_row { "folder.reorderFavorite" } else { "folder.reorder" };
            for (label, up) in [(tl!("Move Up"), true), (tl!("Move Down"), false)] {
                let before = move_before(&peers, id, up);
                if ui.add_enabled(before.is_some(), egui::Button::new(label).min_size(vec2(180.0, 0.0))).clicked() {
                    if let Some(before) = before {
                        let _ = app.run(cmd, json!({"id": id, "before": before}));
                    }
                    ui.close();
                }
            }
            ui.separator();
        }
        item(ui, app, tl_later!("New Folder…"), "ui.dialog", json!({"name": "folder", "mode": "new", "parent": id}), takes_folders);
        // (Not one that is or holds what a JMAP account's server has, which stays for now.)
        let renamable = custom && !app.session.mailbox.renaming_stays_on_server(id);
        item(ui, app, tl_later!("Rename Folder…"), "ui.dialog", json!({"name": "folder", "mode": "rename", "id": id}), renamable);
        let deletable = custom && !app.session.mailbox.folder_stays_on_server(id);
        item(ui, app, tl_later!("Delete Folder"), "folder.delete", json!({"id": id}), deletable);
        ui.separator();
        // Its tabs and search take clicks: only a choice, or a click outside, closes it (v0.12.23).
        egui::containers::menu::SubMenuButton::new(tl!("Tag"))
            .config(egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside))
            .ui(ui, |ui| tag_menu(app, ui, id, &tag));
        item(ui, app, tl_later!("Mark All as Read"), "mail.markAllRead", json!({"folder": id}), true);
        // (Not over a shared mailbox's folder: the rules leave its mail alone.)
        let ruled = app.session.mailbox.rules_reach(app.session.mailbox.folder(id).map_or(0, |f| f.account));
        item(
            ui,
            app,
            tl_later!("Rules for This Folder…"),
            "ui.dialog",
            json!({"name": "rules", "destination": id}),
            !app.session.mailbox.is_send_queue(id),
        );
        item(ui, app, tl_later!("Run Rules Now"), "rules.run", json!({"folder": id}), ruled);
        if matches!(kind, FolderKind::Deleted | FolderKind::Junk) {
            let emptiable = !app.session.mailbox.emptying_stays_on_server(id);
            item(ui, app, tl_later!("Empty Folder"), "folder.empty", json!({"id": id}), emptiable);
        }
        ui.separator();
        if from_outlook {
            item(ui, app, tl_later!("Favorites Are Set in Outlook"), "folder.favorite", json!({"id": id}), false);
        } else {
            item(
                ui,
                app,
                if favorite { tl_later!("Remove from Favorites") } else { tl_later!("Add to Favorites") },
                "folder.favorite",
                json!({"id": id}),
                true,
            );
        }
        if recent_row {
            item(ui, app, "Remove from Recent", "folder.recent", json!({"remove": id}), true);
        }
        ui.separator();
        item(ui, app, tl_later!("Properties…"), "folder.properties", json!({"id": id}), true);
    });
}

/// How many emoji a row of the Tag menu has, how big each one's button is, and how many rows show.
const TAG_COLS: usize = 10;
const TAG_CELL: f32 = 32.0;
const TAG_ROWS: f32 = 9.5;

/// A line of the Tag menu's emoji browser: a group's heading, or a row of its emoji (the section,
/// and where the row starts in the list of every emoji shown).
#[derive(Clone, Copy)]
enum TagLine {
    Head(usize),
    Emoji(usize, usize),
}

/// A folder's Tag menu (`emoji_index`, v0.12.22): a browser of every emoji, the suggested ones
/// first, then each group under its name; the tabs above jump to a group and show the one in view;
/// a search over their names; a field for any other tag; and No Tag. The tag a folder has, chosen
/// again, takes it off.
fn tag_menu(app: &mut MailApp, ui: &mut Ui, id: Id, now: &str) {
    use crate::emoji_index::{TABS, Tab};
    let t = Tokens::get(ui.ctx());
    let width = TAG_COLS as f32 * (TAG_CELL + 2.0);
    let state = egui::Id::unique(("tag_index", id));
    // What is typed in the search, and the tab whose group to scroll to.
    let (mut query, mut jump): (String, Option<Tab>) = ui.data_mut(|d| d.get_temp(state)).unwrap_or_default();
    let search = ui.add(egui::TextEdit::singleline(&mut query).hint_text(tl!("Search emoji")).desired_width(width));
    app.hit.add("tagSearch", 0, search.rect);
    let searching = !query.trim().is_empty();

    let list = crate::emoji_index::listed(ui.ctx());
    let sections: Vec<(usize, Vec<(&'static str, &'static str)>)> = if searching {
        vec![(usize::MAX, crate::emoji_index::search(&list, &query))]
    } else {
        TABS.iter().enumerate().map(|(n, (tab, _, _))| (n, crate::emoji_index::in_tab(&list, *tab))).collect()
    };
    let mut lines = Vec::new();
    let mut first = 0;
    for (s, (n, shown)) in sections.iter().enumerate() {
        if *n != usize::MAX {
            lines.push(TagLine::Head(s));
        }
        for row in 0..shown.len().div_ceil(TAG_COLS) {
            lines.push(TagLine::Emoji(s, first + row * TAG_COLS));
        }
        first += shown.len();
    }
    let starts: Vec<usize> = sections.iter().scan(0, |at, (_, shown)| Some(std::mem::replace(at, *at + shown.len()))).collect();

    // The tabs: each group's face; the one in view is marked, and a click scrolls to it.
    let in_view_key = egui::Id::unique(("tag_in_view", id));
    let in_view: usize = ui.data_mut(|d| d.get_temp(in_view_key)).unwrap_or_default();
    if !searching {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let tab_w = (width - 2.0 * (TABS.len() as f32 - 1.0)) / TABS.len() as f32;
            for (n, (tab, face, name)) in TABS.iter().enumerate() {
                let (r, button) = ui.allocate_exact_size(vec2(tab_w, 30.0), Sense::click());
                let on = n == in_view;
                if button.hovered() {
                    ui.painter().rect_filled(r, 4.0, t.hover);
                }
                ui.painter().text(r.center() - vec2(0.0, 1.0), egui::Align2::CENTER_CENTER, *face, egui::FontId::proportional(17.0), t.text);
                if on {
                    ui.painter().rect_filled(Rect::from_min_max(pos2(r.min.x + 4.0, r.max.y - 3.0), pos2(r.max.x - 4.0, r.max.y)), 1.5, t.accent);
                }
                let label = tl!(*name);
                button.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, label));
                let button = button.on_hover_text(label);
                app.hit.add("tagTab", n as Id, button.rect);
                if button.clicked() {
                    jump = Some(*tab);
                }
            }
        });
        ui.painter().hline(ui.min_rect().x_range(), ui.cursor().min.y, egui::Stroke::new(1.0, t.border));
        ui.add_space(4.0);
    }
    if lines.is_empty() {
        ui.label(tl!("No emoji found"));
    }

    let mut scroll = egui::ScrollArea::vertical().id_salt(("tag_rows", id)).max_height(TAG_CELL * TAG_ROWS).min_scrolled_width(width);
    if let Some(tab) = jump.take() {
        let row_h = TAG_CELL + ui.spacing().item_spacing.y;
        if let Some(line) = lines.iter().position(|l| matches!(l, TagLine::Head(s) if TABS[sections[*s].0].0 == tab)) {
            scroll = scroll.vertical_scroll_offset(line as f32 * row_h);
        }
    }
    let mut hovered: Option<(&str, &str)> = None;
    let mut top_section = None;
    let mut last_section = None;
    let out = scroll.show_rows(ui, TAG_CELL, lines.len(), |ui, range| {
        for line in &lines[range] {
            match *line {
                TagLine::Head(s) => {
                    top_section.get_or_insert(s);
                    last_section = Some(s);
                    let (r, _) = ui.allocate_exact_size(vec2(width, TAG_CELL), Sense::hover());
                    let name = tl!(TABS[sections[s].0].2);
                    ui.painter().text(pos2(r.min.x + 4.0, r.max.y - 6.0), egui::Align2::LEFT_BOTTOM, name, bold(12.0), t.group_label);
                }
                TagLine::Emoji(s, start) => {
                    top_section.get_or_insert(s);
                    last_section = Some(s);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let shown = &sections[s].1;
                        let from = start - starts[s];
                        for (k, (tag, name)) in shown.iter().enumerate().skip(from).take(TAG_COLS) {
                            let on = *tag == now;
                            let button = ui.add(
                                egui::Button::new(egui::RichText::new(*tag).size(18.0))
                                    .frame_when_inactive(on)
                                    .selected(on)
                                    .min_size(vec2(TAG_CELL, TAG_CELL - 2.0)),
                            );
                            app.hit.add("tagChoice", (starts[s] + k) as Id, button.rect);
                            if button.hovered() {
                                hovered = Some((*tag, *name));
                            }
                            if button.clicked() {
                                let _ = app.run("folder.tag", json!({"id": id, "tag": if on { "" } else { *tag }}));
                                ui.close();
                            }
                        }
                    });
                }
            }
        }
    });
    // Scrolled to the end, the last groups can't reach the top: the last one in view is marked.
    let at_end = out.state.clamped_offset().y + out.inner_rect.height() >= out.content_size.y - 1.0;
    if let Some(s) = (if at_end { last_section } else { top_section }).filter(|_| !searching) {
        ui.data_mut(|d| d.insert_temp(in_view_key, sections[s].0));
    }
    // What the pointer is on, by name, as a character map shows it.
    ui.horizontal(|ui| {
        ui.set_min_height(26.0);
        if let Some((tag, name)) = hovered {
            ui.label(egui::RichText::new(tag).size(20.0));
            ui.label(egui::RichText::new(name).color(t.text_dim));
        }
    });
    ui.data_mut(|d| d.insert_temp(state, (query, jump)));
    ui.separator();
    let key = egui::Id::unique(("tag_typed", id));
    let mut typed: String = ui.data_mut(|d| d.get_temp(key)).unwrap_or_else(|| now.to_string());
    let field = ui.add(egui::TextEdit::singleline(&mut typed).hint_text(tl!("Or type one, then Enter")).char_limit(16).desired_width(width));
    ui.data_mut(|d| d.insert_temp(key, typed.clone()));
    if field.lost_focus() && crate::keys::enter(ui.ctx(), ui.layer_id()) {
        let _ = app.run("folder.tag", json!({"id": id, "tag": typed}));
        ui.data_mut(|d| d.remove::<String>(key));
        ui.close();
    }
    if ui.add_enabled(!now.is_empty(), egui::Button::new(tl!("No Tag")).min_size(vec2(width, 0.0))).clicked() {
        let _ = app.run("folder.tag", json!({"id": id, "tag": ""}));
        ui.close();
    }
}

/// A row's count, flush right before `right_edge`, in a pill: an unread count in the accent's soft
/// colour, a total (Drafts, Junk) in a quiet grey one. Returns where the row's name has to end.
fn count_pill(painter: &Painter, t: &Tokens, r: Rect, right_edge: f32, count: Count, selected: bool) -> f32 {
    if count == Count::Nothing {
        return right_edge - COUNT_RIGHT;
    }
    let (fill, ink) = if matches!(count, Count::Unread(_)) { (t.accent_soft, t.accent_soft_text) } else { (t.pill_quiet, t.text_dim) };
    let fill = if selected && matches!(count, Count::Unread(_)) { t.surface } else { fill };
    let galley = painter.layout_no_wrap(count.digits(), bold(NAME_SIZE - 1.0), ink);
    let w = (galley.size().x + 12.0).max(20.0);
    let pill = Rect::from_center_size(pos2(right_edge - COUNT_RIGHT + 4.0 - w / 2.0, r.center().y), vec2(w, 18.0));
    painter.rect_filled(pill, 9.0, fill);
    painter.galley(pill.center() - galley.size() / 2.0, galley, ink);
    pill.min.x - 6.0
}

/// The "Unified mailbox" root at the top of the pane, drawn as an account's heading is: a chevron
/// that folds what is under it (kept as an account's is, under
/// [`crate::unified_inbox::UNIFIED_ROOT`]), its name, and a small "+" at its right for a new search
/// folder. Its context menu picks the filing account when there are several. `true` when what is
/// under it shows.
fn unified_heading(app: &mut MailApp, ui: &mut Ui, accounts: &[(Id, String)], current: Option<Id>) -> bool {
    let t = Tokens::get(ui.ctx());
    let id = crate::unified_inbox::UNIFIED_ROOT;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let y = r.min.y + 7.5;
    let plus = Rect::from_center_size(pos2(r.max.x - 14.0, y + 1.0), vec2(20.0, 20.0));
    let head = Rect::from_min_max(r.min, pos2(plus.min.x - 2.0, r.max.y));
    let resp = ui.interact(head, ui.scope_id().with(("heading", id)), Sense::click());
    app.hit.add("folderHeading", id, head);
    app.hit.add("unifiedFiling", 0, head);
    let open = !app.ui.collapsed_folders.contains(&id);
    let name = tl!("Unified mailbox");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    if resp.has_focus() {
        ui.painter().rect_stroke(highlight(head), 4.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
    }
    chevron(ui.painter(), pos2(r.min.x + CHEVRON_X, y + 2.0), HEADING_CHEVRON, open, if resp.hovered() { t.accent_text } else { t.text });
    paint_clipped(ui.painter(), pos2(r.min.x + NAME_LEFT, y), (head.max.x - r.min.x - NAME_LEFT - 4.0).max(0.0), name, regular(HEADING_SIZE), t.text);
    // "New search folder…": a small icon button beside the name.
    let presp = ui.interact(plus, ui.scope_id().with("unified_new_search_folder"), Sense::click());
    app.hit.add("searchFolderNew", 0, plus);
    presp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("New search folder")));
    if presp.hovered() {
        ui.painter().rect_filled(plus, 4.0, if presp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    if presp.has_focus() {
        ui.painter().rect_stroke(plus, 4.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
    }
    icons::paint(ui.painter(), plus.shrink(3.0), "plus", if presp.hovered() { t.accent_text } else { t.icon }, t.accent);
    if presp.on_hover_text(tl!("New search folder")).clicked() {
        let _ = app.run("ui.dialog", json!({"name":"searchFolder"}));
    }
    let resp = resp.on_hover_text(tl!(
        "Every account's Inbox, all your mail in one list, your folders for all accounts (Folders lists what is not filed yet) and your search folders. Right-click to choose the filing account."
    ));
    if resp.clicked() {
        let _ = app.execute("ui.toggleFolder", json!({"id": id}));
    }
    resp.context_menu(|ui| crate::unified_inbox::root_menu(app, ui, accounts, current));
    open
}

/// The unified mailbox's "Folders": the head of the folders for every account, which lists the mail
/// of every account that is not filed in them yet. They are kept in the filing account's Inbox,
/// but this row is not that Inbox (v0.12.24): no star, tag or rules of its, nor its menu or
/// mail dropped on it; its menu makes a folder in it and is the unified mailbox's own.
#[allow(clippy::too_many_arguments)]
fn unified_folders_row(
    app: &mut MailApp,
    ui: &mut Ui,
    inbox: &Folder,
    has_children: bool,
    count: Count,
    selected: bool,
    accounts: &[(Id, String)],
    current: Option<Id>,
) {
    let t = Tokens::get(ui.ctx());
    let name = tl!("Folders");
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("unified_folders", inbox.id)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    app.hit.add("unifiedFolder", inbox.id, r);
    let row = highlight(r).shrink2(vec2(0.0, ROW_HIGHLIGHT_INSET));
    if selected {
        ui.painter().rect_filled(row, 6.0, t.accent_soft);
        ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 1.5, row.center().y), vec2(3.0, 14.0)), 1.5, t.selection_edge);
    } else if resp.hovered() {
        ui.painter().rect_filled(row, 6.0, t.hover_nav);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(row, 6.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
    }
    if has_children {
        let tr = Rect::from_min_max(pos2(r.min.x + HIGHLIGHT_LEFT, r.min.y), pos2(r.min.x + NAME_LEFT, r.max.y));
        let tresp = ui.interact(tr, ui.scope_id().with(("unified_folders_twisty", inbox.id)), Sense::click());
        let open = !app.ui.collapsed_unified.contains(&inbox.id);
        chevron(ui.painter(), pos2(r.min.x + CHEVRON_X, r.center().y), ROW_CHEVRON, open, if tresp.hovered() { t.accent_text } else { t.text });
        if tresp.clicked() {
            let _ = app.execute("ui.toggleFolder", json!({"id": inbox.id, "unified": true}));
        }
    }
    let name_end = count_pill(ui.painter(), &t, r, r.max.x, count, selected);
    paint_clipped(
        ui.painter(),
        pos2(r.min.x + NAME_LEFT, r.center().y),
        (name_end - r.min.x - NAME_LEFT).max(0.0),
        name,
        if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) },
        if selected { t.accent_soft_text } else { t.text },
    );
    if resp.clicked() {
        let _ = app.run("view.filing", json!({"account": inbox.account, "folder": inbox.id}));
    }
    let id = inbox.id;
    resp.context_menu(|ui| {
        if ui.add(egui::Button::new(tl!("New Folder…")).min_size(vec2(180.0, 0.0))).clicked() {
            let _ = app.run("ui.dialog", json!({"name": "folder", "mode": "new", "parent": id}));
            ui.close();
        }
        ui.separator();
        crate::unified_inbox::root_menu(app, ui, accounts, current);
    });
}

/// The unified mailbox's Inbox (`allInboxes`: every account's Inbox together, `view.allInboxes`)
/// or its All mail (`allMail`: every folder's mail together, newest first, `view.allMail`), with
/// `unread` mail counted.
fn unified_list_row(app: &mut MailApp, ui: &mut Ui, all_mail: bool, unread: usize, selected: bool) {
    let t = Tokens::get(ui.ctx());
    let (key, command, name, hint) = if all_mail {
        ("allMail", "view.allMail", tl!("All mail"), tl!("Every new email, whatever folder it went to"))
    } else {
        ("allInboxes", "view.allInboxes", tl!("Inbox"), tl!("All inboxes"))
    };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("unified_list", key)), Sense::click());
    app.hit.add(key, 0, r);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, hint));
    let row = highlight(r);
    if selected {
        ui.painter().rect_filled(row, 6.0, t.accent_soft);
        ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 1.5, row.center().y), vec2(3.0, 14.0)), 1.5, t.selection_edge);
    } else if resp.hovered() {
        ui.painter().rect_filled(row, 6.0, t.hover_nav);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(row, 6.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
    }
    let count = if unread > 0 { Count::Unread(unread) } else { Count::Nothing };
    let name_end = count_pill(ui.painter(), &t, r, r.max.x, count, selected);
    paint_clipped(
        ui.painter(),
        pos2(r.min.x + NAME_LEFT, r.center().y),
        (name_end - r.min.x - NAME_LEFT).max(0.0),
        name,
        if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) },
        if selected { t.accent_soft_text } else { t.text },
    );
    if resp.on_hover_text(hint).clicked() {
        let _ = app.run(command, json!({}));
    }
}

/// What the window calls one of Gmail's categories (#911).
pub(crate) fn gmail_name(category: GmailCategory) -> &'static str {
    match category {
        GmailCategory::Primary => tl!("Primary"),
        GmailCategory::Social => tl!("Social"),
        GmailCategory::Promotions => tl!("Promotions"),
        GmailCategory::Updates => tl!("Updates"),
        GmailCategory::Forums => tl!("Forums"),
    }
}

/// What a row of one of Gmail's categories is known by to `ui.inspect`, with its Inbox's id.
fn gmail_hit(category: GmailCategory) -> &'static str {
    match category {
        GmailCategory::Primary => "gmailPrimary",
        GmailCategory::Social => "gmailSocial",
        GmailCategory::Promotions => "gmailPromotions",
        GmailCategory::Updates => "gmailUpdates",
        GmailCategory::Forums => "gmailForums",
    }
}

/// The row of one of Gmail's categories under a Gmail account's Inbox (#911), a level further in
/// than the Inbox (`depth`): its name and its unread count, `selected` when the list shows it.
/// It is a view of the Inbox, no folder: a click shows its mail, and it has nothing to rename or
/// delete, so no menu. Mail let go over it is moved to that category where moves between Gmail's
/// categories are on ([`crate::gmail_move`]), and stays where it was otherwise.
fn gmail_row(app: &mut MailApp, ui: &mut Ui, inbox: Id, depth: usize, category: GmailCategory, unread: usize, selected: bool) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("gmail", inbox, category.code())), Sense::click());
    app.hit.add(gmail_hit(category), inbox, r);
    let row = highlight(r).shrink2(vec2(0.0, ROW_HIGHLIGHT_INSET));
    row_backdrop(ui, &t, row, selected, &resp);
    let x = r.min.x + depth.min(MAX_INDENT_DEPTH) as f32 * INDENT;
    let y = r.center().y;
    let mut name_end = r.max.x - COUNT_RIGHT;
    if unread > 0 {
        let fill = if selected { t.surface } else { t.accent_soft };
        let galley = ui.painter().layout_no_wrap(unread.to_string(), bold(NAME_SIZE - 1.0), t.accent_soft_text);
        let w = (galley.size().x + 12.0).max(20.0);
        let pill = Rect::from_center_size(pos2(r.max.x - COUNT_RIGHT + 4.0 - w / 2.0, r.center().y), vec2(w, 18.0));
        ui.painter().rect_filled(pill, 9.0, fill);
        ui.painter().galley(pill.center() - galley.size() / 2.0, galley, t.accent_soft_text);
        name_end = pill.min.x - 6.0;
    }
    let name = gmail_name(category);
    let font = if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) };
    paint_clipped(
        ui.painter(),
        pos2(x + NAME_LEFT, y),
        (name_end - x - NAME_LEFT).max(0.0),
        name,
        font,
        if selected { t.accent_soft_text } else { t.text },
    );
    // Read out as its name and how many of its messages are unread.
    let said = if unread > 0 { format!("{name}, {}", tln!(unread, "{n} unread", "{n} unread")) } else { name.to_string() };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::TreeItem, true, selected, &said));
    if resp.clicked() {
        let _ = app.run("folder.select", json!({"id": inbox, "gmail": category.code()}));
    }
    crate::gmail_move::drop_target(app, ui, &resp, row, category);
}

/// What the window calls every account's folder of `kind` together (#625).
pub(crate) fn unified_name(kind: FolderKind) -> &'static str {
    match kind {
        FolderKind::Drafts => tl!("All Drafts"),
        FolderKind::Sent => tl!("All Sent Items"),
        FolderKind::Deleted => tl!("All Deleted Items"),
        FolderKind::Archive => tl!("All Archives"),
        _ => tl!("All Inboxes"),
    }
}

/// A row under All Accounts (#625): every account's folder of `kind` in one list, counted as
/// one of them would be with all of theirs, `selected` while the list shows it. A view over the
/// accounts' folders, no folder of its own: nothing to rename, delete or drop mail on.
fn unified_row(app: &mut MailApp, ui: &mut Ui, kind: FolderKind, count: Count, selected: bool) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(r, ui.scope_id().with(("unified", kind.code())), Sense::click());
    app.hit.add("allAccountsFolder", mailcraft_engine::unified::KINDS.iter().position(|k| *k == kind).unwrap_or(0) as Id, r);
    let row = highlight(r).shrink2(vec2(0.0, ROW_HIGHLIGHT_INSET));
    row_backdrop(ui, &t, row, selected, &resp);
    let x = r.min.x;
    let name_end = count_pill(ui.painter(), &t, r, r.max.x, count, selected);
    let name = unified_name(kind);
    let font = if selected { bold(NAME_SIZE) } else { regular(NAME_SIZE) };
    let ink = if selected { t.accent_soft_text } else { t.text };
    paint_clipped(ui.painter(), pos2(x + NAME_LEFT, r.center().y), (name_end - x - NAME_LEFT).max(0.0), name, font, ink);
    let said = match count {
        Count::Unread(n) => format!("{name}, {}", tln!(n, "{n} unread", "{n} unread")),
        _ => name.to_string(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::TreeItem, true, selected, &said));
    if resp.clicked() {
        let _ = app.run("view.allAccounts", json!({"kind": kind.code()}));
    }
}

/// A sentence under the Favorites heading that is longer than a row: wrapped to the pane, three
/// rows at most, in the quiet colour. Where it was put.
fn quiet_lines(ui: &mut Ui, text: &str) -> Rect {
    let t = Tokens::get(ui.ctx());
    let width = (ui.available_width() - NAME_LEFT - COUNT_RIGHT).max(40.0);
    let mut job = egui::text::LayoutJob::simple(text.to_string(), regular(NAME_SIZE), t.text_dim, width);
    job.wrap.max_rows = 3;
    let said = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), (said.size().y + 8.0).max(ROW_H)), Sense::hover());
    ui.painter().galley(pos2(r.min.x + NAME_LEFT, r.min.y + 4.0), said, t.text_dim);
    r
}

/// A link under such a sentence, a row of its own: what the owner can do about it.
fn quiet_link(app: &mut MailApp, ui: &mut Ui, account: Id, step: crate::favorites_sync::Step) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let resp = ui.interact(highlight(r), ui.scope_id().with(("favorites_step", account, step.code())), Sense::click());
    let kind = match step {
        crate::favorites_sync::Step::CopyMessage => "favoritesCopyMessage",
        crate::favorites_sync::Step::Help => "favoritesHelp",
    };
    app.hit.add(kind, account, r);
    if resp.hovered() {
        ui.painter().rect_filled(highlight(r), 0.0, t.hover_nav);
    }
    let width = (r.max.x - COUNT_RIGHT - r.min.x - NAME_LEFT).max(0.0);
    paint_clipped(ui.painter(), pos2(r.min.x + NAME_LEFT, r.center().y), width, tl!(step.label()), regular(NAME_SIZE), t.accent_text);
    if resp.on_hover_text(tl!(step.about())).clicked() {
        crate::favorites_sync::step_taken(app, account, step);
    }
}

/// What is said under the Favorites heading, when there is something to say of an account's
/// Favorites in Outlook ([`crate::favorites_sync`]): the offer to show them, why they can't be
/// had, or why the last look came to nothing, with "Try again" after it: one row. When what
/// stands in the way is somebody else's to change (an administrator has to approve it), the
/// sentence that says so, and under it a row for each thing the owner can do about it.
fn favorites_note(app: &mut MailApp, ui: &mut Ui, account: Id, note: &Note) {
    let t = Tokens::get(ui.ctx());
    if matches!(note, Note::Approval) {
        let r = quiet_lines(ui, tl!(note.text()));
        app.hit.add("favoritesNote", account, r);
        let about = tl!(
            "Your organisation keeps this permission for its administrators to give. Once one of them has approved it, choose it again here. Until then the Favorites kept in MailCraft are shown."
        );
        ui.interact(r, ui.scope_id().with(("favorites_note", account)), Sense::hover()).on_hover_text(about);
        for step in app.favorites_steps(note) {
            quiet_link(app, ui, account, step);
        }
        return;
    }
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let y = r.center().y;
    let left = r.min.x + NAME_LEFT;
    let width = (r.max.x - COUNT_RIGHT - left).max(0.0);
    let font = regular(NAME_SIZE);
    match note {
        // The whole line is the offer.
        Note::Offer => {
            let resp = ui.interact(highlight(r), ui.scope_id().with(("favorites_offer", account)), Sense::click());
            app.hit.add("favoritesOffer", account, r);
            if resp.hovered() {
                ui.painter().rect_filled(highlight(r), 0.0, t.hover_nav);
            }
            paint_clipped(ui.painter(), pos2(left, y), width, tl!("Show Outlook's Favorites"), font, t.accent_text);
            // Signing in with Microsoft once more, for that alone.
            let about = tl!(
                "Sign in with Microsoft once more to let MailCraft read the folders you keep under Favorites in Outlook. They are only read, never changed, and everything else stays as it is."
            );
            if resp.on_hover_text(about).clicked()
                && let Err(e) = app.run("ui.connectFavorites", json!({"account": account}))
            {
                app.status(e);
            }
        }
        // (Said above, in more than a row.)
        Note::Approval => {}
        Note::Refused(why) => {
            let resp = ui.interact(highlight(r), ui.scope_id().with(("favorites_note", account)), Sense::hover());
            app.hit.add("favoritesNote", account, r);
            paint_clipped(ui.painter(), pos2(left, y), width, tl!(why), font, t.text_dim);
            resp.on_hover_text(tl!(
                "Outlook's Favorites can't be shown for this account, so the ones kept in MailCraft are. File › Options can ask again."
            ));
        }
        // Why, in a few words, and the way to ask again right after it.
        Note::Failed(why) => {
            app.hit.add("favoritesNote", account, r);
            let again = tl!("Try again");
            let again_w = crate::theme::text_width(ui.painter(), again, &font, 0.0, t.accent_text);
            let used = paint_clipped(ui.painter(), pos2(left, y), (width - again_w - 10.0).max(0.0), tl!(why), font.clone(), t.text_dim);
            let link = Rect::from_min_size(pos2(left + used + if used > 0.0 { 10.0 } else { 0.0 }, r.min.y), vec2(again_w, ROW_H));
            let resp = ui.interact(link, ui.scope_id().with(("favorites_retry", account)), Sense::click());
            app.hit.add("favoritesRetry", account, link);
            let ink = if resp.hovered() { t.accent } else { t.accent_text };
            ui.painter().text(pos2(link.min.x, y), Align2::LEFT_CENTER, again, font, ink);
            if resp.on_hover_text(tl!("Read the Favorites from Outlook again now")).clicked()
                && let Err(e) = app.run("ui.favorites", json!({"account": account, "refresh": true}))
            {
                app.status(e);
            }
        }
    }
}

/// The pane as a sheet lying on the window: the panel colour with a line along its top and left,
/// rounded where the two meet beside the module bar and under the ribbon.
fn sheet(painter: &Painter, r: Rect, t: &Tokens) {
    let radius = f32::from(SHEET_CORNER);
    painter.rect_filled(r, CornerRadius { nw: SHEET_CORNER, ne: 0, sw: 0, se: 0 }, t.panel);
    // Up the left edge, round the corner, along the top: half a pixel in, so the line is crisp.
    let (left, top) = (r.min.x + 0.5, r.min.y + 0.5);
    let centre = pos2(left + radius, top + radius);
    let mut line = vec![pos2(left, r.max.y)];
    line.extend((0..=8).map(|i| {
        let a = (180.0 + 90.0 * i as f32 / 8.0).to_radians();
        pos2(centre.x + radius * a.cos(), centre.y + radius * a.sin())
    }));
    line.push(pos2(r.max.x, top));
    painter.add(Shape::line(line, Stroke::new(1.0, t.border_strong)));
}

/// The special folders the pane's top row has a button for, in this order (v0.12.12).
const SPECIAL: [FolderKind; 6] =
    [FolderKind::Drafts, FolderKind::Sent, FolderKind::Deleted, FolderKind::Junk, FolderKind::Archive, FolderKind::Outbox];
/// The side of a special folder's button.
const SPECIAL_W: f32 = 24.0;

/// The rows of the unified tree without the special folders and what is under them: the buttons
/// over the pane open those, for every account at once.
fn without_special(rows: Vec<(Folder, usize, bool)>) -> Vec<(Folder, usize, bool)> {
    let mut under: Option<usize> = None;
    rows.into_iter()
        .filter(|(f, depth, _)| {
            if under.is_some_and(|d| *depth > d) {
                return false;
            }
            under = None;
            // The server's own folders (Conversation History, Sync Issues and its three, Scheduled,
            // Clutter…, and Gmail's system labels such as Account Events) stay in their account's
            // tab, out of the unified tree (v0.12.13). Not the Inbox, which Outlook counts among
            // its own too: the filing tree is under it (v0.12.16).
            let custom = f.kind == FolderKind::Custom && !f.server_owned() && !gmail_system(&f.remote);
            if f.kind == FolderKind::Inbox || custom {
                true
            } else {
                under = Some(*depth);
                false
            }
        })
        .collect()
}

/// The unified tree with everything under Folders (the filing account's Inbox): Outlook keeps
/// folders such as Legacy beside its Inbox, but here they are the owner's folders like the rest
/// (v0.12.17). Folding Folders folds them too.
fn under_folders(rows: Vec<(Folder, usize, bool)>, folded: &[Id]) -> Vec<(Folder, usize, bool)> {
    let Some(at) = rows.iter().position(|(f, depth, _)| *depth == 0 && f.kind == FolderKind::Inbox) else {
        return rows;
    };
    let mut rows = rows;
    let mut inbox: Vec<_> = rows.drain(at..).collect();
    let end = inbox.iter().skip(1).position(|(_, depth, _)| *depth == 0).map_or(inbox.len(), |i| i + 1);
    let after: Vec<_> = inbox.drain(end..).collect();
    let others: Vec<_> = rows.into_iter().chain(after).collect();
    if others.is_empty() {
        return inbox;
    }
    inbox[0].2 = true;
    if !folded.contains(&inbox[0].0.id) {
        inbox.extend(others.into_iter().map(|(f, depth, kids)| (f, depth + 1, kids)));
    }
    inbox
}

/// A label Gmail keeps in its own `[Gmail]` (`[Google Mail]` in some countries) folder, such as
/// Account Events: Gmail's, not the owner's.
fn gmail_system(remote: &str) -> bool {
    ["[Gmail]/", "[Google Mail]/"].iter().any(|prefix| remote.starts_with(prefix))
}

/// Whose special folders the buttons open: the account in view's tab, else every account's.
fn special_account(app: &MailApp) -> Option<Id> {
    match current_tab(app) {
        NavTab::Account(id) if app.ui.folder_tabs => Some(id),
        _ => None,
    }
}

/// The special folders' buttons, with what each says: the folder's kind, the number on it (how
/// many drafts, how many in the Outbox, how many unread in Junk; 0 none), and whether it is in
/// view. The Outbox has one only while something waits in it.
fn special_buttons(app: &mut MailApp) -> Vec<(FolderKind, usize, bool)> {
    let account = special_account(app);
    let counts = app.session.folder_counts();
    let view = &app.session.view;
    let in_view = |kind: FolderKind| match account {
        None => view.scope == mailcraft_engine::Scope::Inboxes && view.kind == Some(kind) && view.search_folder.is_none(),
        Some(a) => {
            view.scope != mailcraft_engine::Scope::Inboxes
                && view.filing.is_none()
                && app.session.mailbox.folder(view.folder).is_some_and(|f| f.account == a && f.kind == kind)
        }
    };
    SPECIAL
        .into_iter()
        .filter_map(|kind| {
            let folders: Vec<Id> =
                app.session.mailbox.folders.iter().filter(|f| f.kind == kind && account.is_none_or(|a| f.account == a)).map(|f| f.id).collect();
            if folders.is_empty() {
                return None;
            }
            let total = folders.iter().fold(0usize, |n, f| n.saturating_add(counts.total(*f)));
            let unread = folders.iter().fold(0usize, |n, f| n.saturating_add(counts.unread(*f)));
            let number = match kind {
                FolderKind::Drafts | FolderKind::Outbox => total,
                FolderKind::Junk => unread,
                _ => 0,
            };
            (kind != FolderKind::Outbox || total > 0).then_some((kind, number, in_view(kind)))
        })
        .collect()
}

/// The title of the view of one special folder of every account: "Drafts (all accounts)".
pub(crate) fn all_of_title(kind: FolderKind) -> String {
    special_tip(kind, None)
}

/// What a special folder's button says under the pointer: "Drafts (all accounts)", or the folder's
/// name for one account.
fn special_tip(kind: FolderKind, account: Option<Id>) -> String {
    let name = crate::i18n::current().folder(kind.default_name());
    match account {
        None => crate::i18n::fmt(tl!("{folder} (all accounts)"), &[("folder", name)]),
        Some(_) => name.to_string(),
    }
}

/// One special folder's button in `r`: its icon, its number in a small badge, and a click opens it.
fn special_button(app: &mut MailApp, ui: &mut Ui, r: Rect, (kind, number, on): (FolderKind, usize, bool), t: &Tokens) {
    let account = special_account(app);
    let resp = ui.interact(r, ui.scope_id().with(("special_folder", kind.code())), Sense::click());
    if on {
        ui.painter().rect_filled(r, 4.0, t.checked);
    } else if resp.hovered() {
        ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), kind.code(), t.icon, t.accent);
    if number > 0 {
        let text = if number > 99 { "99+".to_string() } else { number.to_string() };
        let galley = ui.painter().layout_no_wrap(text, regular(8.5), t.on_accent);
        let w = (galley.size().x + 5.0).max(11.0);
        let badge = Rect::from_min_size(pos2(r.max.x - w + 2.0, r.min.y - 1.0), vec2(w, 11.0));
        ui.painter().rect_filled(badge, 5.5, t.accent);
        ui.painter().galley(badge.center() - galley.size() / 2.0, galley, t.on_accent);
    }
    app.hit.add("specialFolder", SPECIAL.iter().position(|k| *k == kind).unwrap_or(0) as u64, r);
    let tip = match number {
        0 => special_tip(kind, account),
        n => format!("{} · {n}", special_tip(kind, account)),
    };
    if resp.on_hover_text(tip).clicked() {
        let _ = match account {
            None => app.run("view.allOf", json!({"kind": kind.code()})),
            Some(a) => app.run("folder.select", json!({"kind": kind.code(), "account": a})),
        };
    }
}

/// The top of the pane: the special folders' buttons (v0.12.12), and the button that puts the pane
/// away.
fn pane_top(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    // The line along the pane's top, and a little air under it.
    ui.add_space(4.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let mut x = r.min.x + 6.0;
    for b in special_buttons(app) {
        if x + SPECIAL_W > r.max.x - 28.0 {
            break;
        }
        special_button(app, ui, Rect::from_min_size(pos2(x, r.center().y - SPECIAL_W / 2.0), vec2(SPECIAL_W, SPECIAL_W)), b, t);
        x += SPECIAL_W + 2.0;
    }
    let button = Rect::from_center_size(pos2(r.max.x - 14.0, r.center().y), vec2(20.0, 20.0));
    let resp = ui.interact(button, ui.scope_id().with("hide_pane"), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(button, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let c = button.center();
    ui.painter().add(Shape::line(vec![pos2(c.x + 1.75, c.y - 3.4), pos2(c.x - 1.65, c.y), pos2(c.x + 1.75, c.y + 3.4)], Stroke::new(1.5, t.text)));
    app.hit.add("folderPaneHide", 0, button);
    if resp.on_hover_text(tl!("Hide the folder pane")).clicked() {
        let _ = app.run("view.folderPane", json!({"value": false}));
    }
}

/// What is left of the folder pane while it is hidden: a narrow strip with the button that
/// brings it back, where the button that hid it was.
pub fn folder_pane_strip(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::left("folders_hidden").exact_size(STRIP_W).resizable(false).frame(egui::Frame::NONE.fill(t.canvas)).show(ui, |ui| {
        sheet(ui.painter(), ui.max_rect(), &t);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.add_space(4.0);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
        let button = Rect::from_center_size(r.center(), vec2(20.0, 20.0));
        let resp = ui.interact(button, ui.scope_id().with("show_pane"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(button, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
        }
        let c = button.center();
        ui.painter()
            .add(Shape::line(vec![pos2(c.x - 1.75, c.y - 3.4), pos2(c.x + 1.65, c.y), pos2(c.x - 1.75, c.y + 3.4)], Stroke::new(1.5, t.text)));
        app.hit.add("folderPaneShow", 0, button);
        if resp.on_hover_text(tl!("Show the folder pane")).clicked() {
            let _ = app.run("view.folderPane", json!({"value": true}));
        }
        // The special folders' buttons, one under the other (v0.12.12).
        ui.add_space(6.0);
        for b in special_buttons(app) {
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), SPECIAL_W + 4.0), Sense::hover());
            special_button(app, ui, Rect::from_center_size(row.center(), vec2(SPECIAL_W, SPECIAL_W)), b, &t);
        }
    });
}

/// The folder pane: the unified mailbox, the favourites, then every account's folders; with
/// [`crate::UiState::folder_tabs`], one of them at a time, picked by the tabs down its left edge.
pub fn folder_pane(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let panel = egui::Panel::left("folders")
        .default_size(app.ui.folder_pane_width)
        .size_range(160.0..=420.0)
        .resizable(true)
        // In the dark the pane and the list beside it are told apart by their own colours; on a
        // light window they are nearly one white, and a line parts them.
        .show_separator_line(t.pane_rule)
        .frame(egui::Frame::NONE.fill(t.canvas));
    let shown = panel.show(ui, |ui| {
        // (After the sheet: the pane's background would paint over the row.)
        sheet(ui.painter(), ui.max_rect(), &t);
        module_foot(app, ui);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        // The unified mailbox at the top: every Inbox, the filing account's real tree for all
        // accounts, and the saved search folders.
        let filing_accounts = crate::unified_inbox::filing_accounts(app);
        let filing_account = crate::unified_inbox::filing_account(app, &filing_accounts);
        let tabs = app.ui.folder_tabs;
        let tab = current_tab(app);
        if tabs {
            egui::Panel::left("folder_tabs")
                .exact_size(TAB_W)
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE)
                .show(ui, |ui| tab_strip(app, ui, &t, tab, &filing_accounts, filing_account));
        }
        pane_top(app, ui, &t);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            // (Counted once per change to the mailbox, not at every frame: `Session::folder_counts`.)
            let counts = app.session.folder_counts();
            // What is listed: the unified mailbox, the favourites, and under each account what
            // isn't folded away; with tabs, only what the tab in view has, and nothing folded
            // under a heading (there are none).
            let folded = app.ui.collapsed_folders.clone();
            let shows = |section: NavTab| !tabs || tab == section;
            if shows(NavTab::Unified) && (tabs || unified_heading(app, ui, &filing_accounts, filing_account)) {
                let view = &app.session.view;
                let unified_list =
                    view.scope == mailcraft_engine::Scope::Inboxes && view.search_folder.is_none() && view.filing.is_none() && view.kind.is_none();
                let all_mail = view.all_mail;
                let unread = app
                    .session
                    .mailbox
                    .folders
                    .iter()
                    .filter(|f| f.kind == FolderKind::Inbox)
                    .fold(0usize, |total, f| total.saturating_add(counts.unread(f.id)));
                unified_list_row(app, ui, false, unread, unified_list && !all_mail);
                // A ledger of what came in lately, wherever it went: no count, since filed mail is
                // often left unread.
                unified_list_row(app, ui, true, 0, unified_list && all_mail);
                if let Some(account) = filing_account {
                    // Its Inbox counts what is not filed in its tree yet, from every account.
                    let (total, unread) = mailcraft_engine::cmd::filing::unfiled_folders(&app.session, account)
                        .into_iter()
                        .fold((0usize, 0usize), |(total, unread), id| {
                            (total.saturating_add(counts.total(id)), unread.saturating_add(counts.unread(id)))
                        });
                    let mut tree = trees(&app.session.mailbox, |a| a == account);
                    // (Folded apart from the account's own tree.)
                    let folded = app.ui.collapsed_unified.clone();
                    let rows = under_folders(without_special(visible_in(tree.remove(&account).unwrap_or_default(), &folded)), &folded);
                    let in_view = app.session.view.filing.is_some() && app.session.view.search_folder.is_none();
                    for (f, depth, has_children) in &rows {
                        // (A row out of sight takes its place and is not drawn, as in an account's tree.)
                        let top = ui.cursor().min.y;
                        let clip = ui.clip_rect();
                        if top > clip.max.y || top + ROW_H < clip.min.y {
                            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                            app.hit.add("unifiedFolder", f.id, r);
                            continue;
                        }
                        let count = if f.kind == FolderKind::Inbox {
                            Count::of(f.kind, total, unread)
                        } else {
                            let held = counts.folder(f.id);
                            Count::of(f.kind, held.total, held.unread)
                        };
                        let selected = in_view && f.id == app.session.view.folder;
                        if *depth == 0 && f.kind == FolderKind::Inbox {
                            unified_folders_row(app, ui, f, *has_children, count, selected, &filing_accounts, filing_account);
                        } else {
                            folder_row(app, ui, f, *depth, *has_children, "unified", count, selected);
                        }
                    }
                }
                crate::search_folders::rows(app, ui);
            }
            let aliases = app.session.aliases();
            let alias_query = mailcraft_model::search::Query::parse(&app.session.view.search);
            let selected_alias = (app.session.view.scope == mailcraft_engine::Scope::Mailbox).then(|| alias_query.receiving_alias()).flatten();
            let count = |f: &Folder| {
                let held = counts.folder(f.id);
                Count::of(f.kind, held.total, held.unread)
            };
            // The favourites as the engine lists them: Outlook's own, in Outlook's order, for an
            // account whose server has them; else the ones that are marked here.
            let favorites: Vec<Folder> =
                mailcraft_engine::cmd::folder::favorites(&app.session).into_iter().filter_map(|id| app.session.mailbox.folder(id).cloned()).collect();
            // And what there is to say of Outlook's, in one quiet line under the heading.
            let note = app.favorites_note();
            // (A shared mailbox right after the account it is opened through, #70.)
            // (The folders are grouped by account once, not once for each account.)
            let mut trees = trees(&app.session.mailbox, |a| if tabs { tab == NavTab::Account(a) } else { !folded.contains(&a) });
            // Each account's heading, or with tabs what its heading would say beside its name
            // (No access, Sign in needed), and its rows.
            let lists: Vec<(Id, String, Option<String>, Vec<(Folder, usize, bool)>)> = app
                .session
                .mailbox
                .accounts_in_order()
                .into_iter()
                .filter(|a| shows(NavTab::Account(a.id)))
                .map(|a| {
                    let rows = visible_in(trees.remove(&a.id).unwrap_or_default(), &folded);
                    let heading = heading_name(app, a);
                    let said = (heading != shown_name(a)).then(|| heading.clone());
                    (a.id, heading, said, rows)
                })
                .collect();
            // The folder in view is highlighted once: under Favorites when it shows there, unless
            // it was picked in its account's list and shows there too. While the list shows one of
            // Gmail's categories of it (#911), that category's row is, and the Inbox's isn't; when
            // they are tabs above the list instead (`view.gmailCategories`), the Inbox's is.
            let gmail_tabs = app.session.view.gmail_tabs;
            let gmail_now = mailcraft_engine::gmail::in_view(&app.session);
            // (Every account's folder of a kind, #625: its row under All Accounts, no folder's.)
            let unified = mailcraft_engine::unified::of(&app.session);
            let current = if (gmail_now.is_some() && !gmail_tabs) || unified.is_some() { 0 } else { app.session.view.folder };
            let gmail_current = app.session.view.folder;
            let favorites_shown = if tabs { tab == NavTab::Favorites } else { !folded.contains(&FAVORITES) };
            let among_favorites = favorites_shown && favorites.iter().any(|f| f.id == current);
            let in_a_list = lists.iter().any(|(.., rows)| rows.iter().any(|(f, _, _)| f.id == current));
            let favorite_selected = app.session.view.filing.is_none() && among_favorites && (tabs || !(in_a_list && picked_in_list(ui)));
            // The owner's own accounts are dragged up or down by their headings (#702), when there
            // are two at least; a shared mailbox goes with its account. (With tabs, by the menus of
            // their tabs.)
            let own = owners(&app.session.mailbox);
            let movable: HashSet<Id> = if own.len() > 1 && !tabs { own.into_iter().collect() } else { HashSet::new() };
            // The unified mailbox's heading sits right under the top of the pane; each later one
            // starts a new block, with a rule above it.
            let head =
                |app: &mut MailApp, ui: &mut Ui, id: Id, name: &str| heading(app, ui, id, name, 2.0 * ROW_H, 34.5, true, movable.contains(&id));
            // Every account's Inbox (Drafts, Sent Items…) together, for an owner with two accounts at
            // least, a shared mailbox counting as one (#625): each kind that one of them has. (In the
            // one-list pane only; with tabs the unified mailbox's tab has them.)
            if !tabs && mailcraft_engine::unified::offered(&app.session.mailbox) && head(app, ui, ALL_ACCOUNTS, tl!("All Accounts")) {
                for kind in mailcraft_engine::unified::KINDS {
                    let folders = mailcraft_engine::unified::folders(&app.session.mailbox, kind);
                    if folders.is_empty() {
                        continue;
                    }
                    let (total, unread) = folders.iter().fold((0usize, 0usize), |(t, u), f| {
                        let held = counts.folder(*f);
                        (t.saturating_add(held.total), u.saturating_add(held.unread))
                    });
                    unified_row(app, ui, kind, Count::of(kind, total, unread), unified == Some(kind));
                }
            }
            let favorites_open =
                if tabs { tab == NavTab::Favorites } else { (!favorites.is_empty() || note.is_some()) && head(app, ui, FAVORITES, tl!("Favorites")) };
            if favorites_open {
                if favorites.is_empty() && note.is_none() {
                    quiet_line(ui, &t, tl!("No favorites yet."));
                    quiet_line(ui, &t, tl!("Right-click a folder to add it."));
                }
                if let Some((account, note)) = &note {
                    favorites_note(app, ui, *account, note);
                }
                // Each with the folders under it once opened there, those folded as in its
                // account's tree (v0.12.17).
                let rows: Vec<(Folder, usize, bool)> = {
                    let folded = app.ui.collapsed_folders.clone();
                    let opened = app.ui.open_favorites.clone();
                    let by_account = self::trees(&app.session.mailbox, |a| favorites.iter().any(|f| f.account == a));
                    let mut rows = Vec::new();
                    for f in &favorites {
                        let tree = by_account.get(&f.account);
                        let under = tree.is_some_and(|tree| tree.get(&Some(f.id)).is_some_and(|list| !list.is_empty()));
                        rows.push((f.clone(), 0, under));
                        if let Some(tree) = tree.filter(|_| under && opened.contains(&f.id)) {
                            list_under(tree, &folded, Some(f.id), 1, &mut rows);
                        }
                    }
                    rows
                };
                for (f, depth, under) in &rows {
                    folder_row(
                        app,
                        ui,
                        f,
                        *depth,
                        *under,
                        "fav",
                        count(f),
                        app.session.view.search_folder.is_none()
                            && app.session.view.scope != mailcraft_engine::Scope::Inboxes
                            && selected_alias.is_none()
                            && favorite_selected
                            && f.id == current,
                    );
                }
            }
            // The folders opened or filed into last (v0.12.14), those not among the Favorites,
            // under their own heading; none in the tabs, nor while there are none.
            let recent: Vec<Folder> = if tabs {
                Vec::new()
            } else {
                mailcraft_engine::cmd::folder::recent(&app.session)
                    .into_iter()
                    .filter(|id| !favorites.iter().any(|f| f.id == *id))
                    .filter_map(|id| app.session.mailbox.folder(id).cloned())
                    .collect()
            };
            if !recent.is_empty() && head(app, ui, RECENT, tl!("Recent")) {
                for f in &recent {
                    folder_row(app, ui, f, 0, false, "recent", count(f), false);
                }
            }
            // Where each of the owner's accounts starts, for a drag of one.
            let mut blocks: Vec<Block> = Vec::new();
            for (account, name, said, rows) in &lists {
                if movable.contains(account) {
                    let top = ui.cursor().min.y;
                    // (On the rule above its heading.)
                    let line = top + 9.5;
                    blocks.push(Block { account: *account, top, line });
                }
                if tabs {
                    if let Some(said) = said {
                        quiet_line(ui, &t, said);
                    }
                } else if !head(app, ui, *account, if name.is_empty() { tl!("Mailbox") } else { name }) {
                    continue;
                }
                if let Some(observed) = aliases.accounts.get(account) {
                    let current_account = app.session.mailbox.folder(current).map(|folder| folder.account);
                    alias_rows(app, ui, *account, observed, if current_account == Some(*account) { selected_alias } else { None });
                }
                for (f, depth, has_children) in rows {
                    // A Gmail account's Inbox has Gmail's categories under it, folded with it,
                    // unless they are tabs above its list.
                    let gmail = !gmail_tabs && f.kind == FolderKind::Inbox && mailcraft_engine::gmail::listed_under(&mut app.session, f.id);
                    // A row out of sight takes its place and is not drawn (#991: thousands of
                    // folders), but is still known where it is.
                    let top = ui.cursor().min.y;
                    let clip = ui.clip_rect();
                    if !gmail && (top > clip.max.y || top + ROW_H < clip.min.y) {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                        app.hit.add("folder", f.id, r);
                        continue;
                    }
                    folder_row(
                        app,
                        ui,
                        f,
                        *depth,
                        *has_children || gmail,
                        "tree",
                        count(f),
                        app.session.view.search_folder.is_none()
                            && app.session.view.filing.is_none()
                            && app.session.view.scope != mailcraft_engine::Scope::Inboxes
                            && selected_alias.is_none()
                            && !favorite_selected
                            && f.id == current,
                    );
                    if gmail && !app.ui.collapsed_folders.contains(&f.id) {
                        for category in GmailCategory::ALL {
                            let unread = counts.gmail(f.id, category).unread;
                            let selected = f.id == gmail_current && gmail_now == Some(category);
                            gmail_row(app, ui, f.id, depth + 1, category, unread, selected);
                        }
                    }
                }
            }
            let end = ui.cursor().min.y;
            account_drop(app, ui, &blocks, end);
            ui.add_space(8.0);
        });
        drag_ghost(app, ui);
    });
    app.ui.folder_pane_width = shown.response.rect.width();
}

/// A quiet line of text in the pane, where a name would start.
fn quiet_line(ui: &mut Ui, t: &Tokens, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    paint_clipped(
        ui.painter(),
        pos2(r.min.x + NAME_LEFT, r.center().y),
        (r.width() - NAME_LEFT - 8.0).max(0.0),
        text,
        regular(NAME_SIZE),
        t.text_dim,
    );
}

/// The width of the folder pane's tabs, down its left edge.
const TAB_W: f32 = 28.0;
/// The size of a tab's name, and the most of the pane's height one name takes.
const TAB_TEXT: f32 = 12.0;
const TAB_TEXT_MAX: f32 = 140.0;
/// Most characters a tab's name has.
const MAX_TAB_NAME: usize = 24;

/// One of the folder pane's tabs ([`crate::UiState::folder_tabs`]): the unified mailbox,
/// Favorites, or an account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NavTab {
    #[default]
    Unified,
    Favorites,
    Account(Id),
}

/// What [`crate::UiState::folder_tabs`] is in settings saved before it was kept: on.
pub(crate) fn tabs_on() -> bool {
    true
}

/// The tab in view: the one picked, unless it is of an account that is no longer there.
fn current_tab(app: &MailApp) -> NavTab {
    match app.ui.nav_tab {
        NavTab::Account(id) if app.session.mailbox.account(id).is_none() => NavTab::Unified,
        tab => tab,
    }
}

/// Saved tab names that are fit to show: at most 200, none too long or with control characters.
pub(crate) fn sane_tab_names(names: &mut std::collections::BTreeMap<Id, String>) {
    names.retain(|_, name| name.chars().count() <= MAX_TAB_NAME && !name.chars().any(char::is_control) && !name.trim().is_empty());
    while names.len() > 200 {
        names.pop_last();
    }
}

/// What an account's tab says: the name the owner gave it, else its address's domain
/// (`Outlook.es`), or where two accounts share one, its first part and a number (`Gmail 1`,
/// `Gmail 2`, in the pane's order).
pub(crate) fn tab_name(app: &MailApp, a: &mailcraft_model::Account) -> String {
    if let Some(name) = app.ui.tab_names.get(&a.id).map(|name| name.trim()).filter(|name| !name.is_empty()) {
        return name.to_string();
    }
    default_tab_name(&app.session.mailbox, a)
}

fn default_tab_name(mb: &Mailbox, a: &mailcraft_model::Account) -> String {
    let domain = |a: &mailcraft_model::Account| a.email.rsplit_once('@').map(|(_, d)| d.trim().to_lowercase()).filter(|d| !d.is_empty());
    let capital = |s: &str| {
        let mut chars = s.chars();
        chars.next().map(|first| first.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
    };
    let Some(own) = domain(a) else {
        return if a.name.trim().is_empty() { tl!("Mailbox").to_string() } else { shown_name(a).to_string() };
    };
    let same: Vec<Id> = mb.accounts_in_order().into_iter().filter(|o| domain(o).as_deref() == Some(own.as_str())).map(|o| o.id).collect();
    if same.len() > 1 {
        let first = own.split('.').next().unwrap_or(&own);
        let n = same.iter().position(|id| *id == a.id).map_or(1, |i| i + 1);
        format!("{} {n}", capital(first))
    } else {
        capital(&own)
    }
}

/// `ui.folderTab`: which tab the folder pane shows.
pub(crate) fn pick_tab(app: &mut MailApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let tab = match p.get("tab") {
        Some(serde_json::Value::String(s)) if s == "unified" => NavTab::Unified,
        Some(serde_json::Value::String(s)) if s == "favorites" => NavTab::Favorites,
        Some(v) => match v.as_u64() {
            Some(id) if app.session.mailbox.account(id).is_some() => NavTab::Account(id),
            _ => return Err("`tab` is \"unified\", \"favorites\" or the id of an account".into()),
        },
        None => return Err("missing `tab`".into()),
    };
    app.ui.nav_tab = tab;
    Ok(json!({"tab": tab, "tabs": app.ui.folder_tabs}))
}

/// `ui.tabName`: the name of an account's tab ("" for its address's domain again).
pub(crate) fn name_tab(app: &mut MailApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let Some(account) = p.get("account").and_then(serde_json::Value::as_u64).filter(|a| app.session.mailbox.account(*a).is_some()) else {
        return Err("no account with that id".into());
    };
    let Some(name) = p.get("name").and_then(serde_json::Value::as_str) else { return Err("missing `name`".into()) };
    if name.chars().count() > MAX_TAB_NAME || name.chars().any(char::is_control) {
        return Err("a tab's name is at most 24 characters".into());
    }
    // (Kept as typed, spaces and all, so a name can be typed a letter at a time; shown trimmed.)
    if name.trim().is_empty() {
        app.ui.tab_names.remove(&account);
    } else {
        app.ui.tab_names.insert(account, name.to_string());
    }
    let shown = app.session.mailbox.account(account).map(|a| tab_name(app, a)).unwrap_or_default();
    Ok(json!({"account": account, "name": shown}))
}

/// The tabs down the folder pane's left edge: the unified mailbox, Favorites, and each account
/// (in the pane's order, with its colour), each name reading upwards. A click shows that tab;
/// a right-click has what its heading's menu had. Mail dragged over a tab opens it, to be let
/// go over one of its folders.
fn tab_strip(app: &mut MailApp, ui: &mut Ui, t: &Tokens, current: NavTab, filing_accounts: &[(Id, String)], filing_account: Option<Id>) {
    let strip = ui.max_rect();
    ui.painter().line_segment([pos2(strip.max.x - 0.5, strip.min.y + 10.0), pos2(strip.max.x - 0.5, strip.max.y - 6.0)], Stroke::new(1.0, t.border));
    let mut tabs: Vec<(NavTab, String, Option<mailcraft_model::Rgb>, String)> = vec![
        (NavTab::Unified, tl!("Unified").to_string(), None, tl!("Unified mailbox").to_string()),
        (NavTab::Favorites, tl!("Favorites").to_string(), None, tl!("Favorites").to_string()),
    ];
    for a in app.session.mailbox.accounts_in_order() {
        let hint = if a.email.is_empty() { shown_name(a).to_string() } else { a.email.clone() };
        tabs.push((NavTab::Account(a.id), tab_name(app, a), Some(a.effective_row_color()), hint));
    }
    let own = owners(&app.session.mailbox);
    let mut y = strip.min.y + 10.0;
    for (tab, name, color, hint) in tabs {
        let on = tab == current;
        let ink = if on { t.accent_soft_text } else { t.text };
        let font = if on { bold(TAB_TEXT) } else { regular(TAB_TEXT) };
        let mut job = egui::text::LayoutJob::single_section(name.clone(), egui::text::TextFormat { font_id: font, color: ink, ..Default::default() });
        job.wrap = egui::text::TextWrapping { max_width: TAB_TEXT_MAX, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let galley = ui.painter().layout_job(job);
        let dot = if color.is_some() { 16.0 } else { 0.0 };
        let r = Rect::from_min_size(pos2(strip.min.x + 3.0, y), vec2(TAB_W - 6.0, galley.size().x + dot + 20.0));
        y = r.max.y + 4.0;
        let key = match tab {
            NavTab::Unified => crate::unified_inbox::UNIFIED_ROOT,
            NavTab::Favorites => FAVORITES,
            NavTab::Account(id) => id,
        };
        app.hit.add("folderTab", key, r);
        let resp = ui.interact(r, ui.scope_id().with(("folder_tab", key)), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, on, &name));
        if on {
            ui.painter().rect_filled(r, 6.0, t.accent_soft);
            ui.painter().rect_filled(
                Rect::from_center_size(pos2(r.min.x + 1.5, r.center().y), vec2(3.0, (r.height() - 12.0).max(6.0))),
                1.5,
                t.selection_edge,
            );
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 6.0, t.hover_nav);
        }
        if resp.has_focus() {
            ui.painter().rect_stroke(r, 6.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
        }
        if let Some(color) = color {
            crate::unified_inbox::swatch(ui.painter(), t, pos2(r.center().x, r.min.y + 13.0), color);
        }
        let centre = pos2(r.center().x, r.max.y - 10.0 - galley.size().x / 2.0);
        let at = centre - galley.size() / 2.0;
        ui.painter().add(Shape::Text(
            egui::epaint::TextShape::new(at, galley, ink).with_angle_and_anchor(-std::f32::consts::FRAC_PI_2, Align2::CENTER_CENTER),
        ));
        // Mail dragged over a tab opens it.
        if !on && resp.dnd_hover_payload::<crate::mail_view::MessageDrag>().is_some() {
            app.ui.nav_tab = tab;
        }
        let resp = resp.on_hover_text(hint);
        if resp.clicked() && !on {
            let _ = app.execute("ui.folderTab", tab_param(tab));
        }
        match tab {
            NavTab::Unified => {
                resp.context_menu(|ui| crate::unified_inbox::root_menu(app, ui, filing_accounts, filing_account));
            }
            NavTab::Favorites => {}
            NavTab::Account(id) => {
                let movable = own.len() > 1 && own.contains(&id);
                resp.context_menu(|ui| {
                    ui.set_min_width(190.0);
                    ui.label(egui::RichText::new(tl!("Tab name")).font(regular(11.0)).weak());
                    let mut typed = app.ui.tab_names.get(&id).cloned().unwrap_or_default();
                    let hint = app.session.mailbox.account(id).map(|a| default_tab_name(&app.session.mailbox, a)).unwrap_or_default();
                    let field = ui.add(egui::TextEdit::singleline(&mut typed).hint_text(hint).char_limit(MAX_TAB_NAME).desired_width(180.0));
                    app.hit.add("tabName", id, field.rect);
                    if field.changed() {
                        let _ = app.execute("ui.tabName", json!({"account": id, "name": typed}));
                    }
                    ui.separator();
                    account_menu(app, ui, id, movable);
                });
            }
        }
    }
}

/// What `ui.folderTab` is told to show `tab`.
fn tab_param(tab: NavTab) -> serde_json::Value {
    match tab {
        NavTab::Unified => json!({"tab": "unified"}),
        NavTab::Favorites => json!({"tab": "favorites"}),
        NavTab::Account(id) => json!({"tab": id}),
    }
}

/// Virtual alias filters belong to an account; they never become move/drop destinations.
fn alias_rows(app: &mut MailApp, ui: &mut Ui, account: Id, observed: &mailcraft_engine::aliases::AccountAliases, selected: Option<&str>) {
    let t = Tokens::get(ui.ctx());
    let expanded = app.ui.expanded_alias_accounts.contains(&account);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("SimpleLogin aliases")));
    app.hit.add("aliasHeading", account, rect);
    if response.hovered() {
        ui.painter().rect_filled(highlight(rect), 6.0, t.hover_nav);
    }
    if response.has_focus() {
        ui.painter().rect_stroke(highlight(rect), 3.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
    }
    chevron(ui.painter(), pos2(rect.min.x + CHEVRON_X, rect.center().y), ROW_CHEVRON, expanded, t.text);
    paint_clipped(
        ui.painter(),
        pos2(rect.min.x + NAME_LEFT, rect.center().y),
        (rect.width() - NAME_LEFT - 12.0).max(0.0),
        tl!("Aliases"),
        regular(NAME_SIZE),
        t.text,
    );
    if response
        .on_hover_text(tl!("SimpleLogin receiving aliases reported in downloaded messages. Filters span this account's folders; no mail is moved."))
        .clicked()
    {
        let _ = app.execute("ui.toggleAliases", json!({"account": account}));
    }
    if !expanded {
        return;
    }
    // Keep long alias collections usable without laying out their every row each frame.
    egui::ScrollArea::vertical().id_salt(("aliasRows", account)).max_height(8.0 * ROW_H).auto_shrink([false, true]).show_rows(
        ui,
        ROW_H,
        observed.aliases.len(),
        |ui, range| {
            for alias in observed.aliases.get(range).unwrap_or_default() {
                let active = selected.is_some_and(|selected| selected.eq_ignore_ascii_case(&alias.address));
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let key = egui::Id::unique(("aliasFilter", account, &alias.address)).value();
                app.hit.add("aliasFilter", key, rect);
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &alias.address));
                let row = highlight(rect);
                if active {
                    ui.painter().rect_filled(row, 6.0, t.accent_soft);
                    ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 1.5, row.center().y), vec2(3.0, 14.0)), 1.5, t.selection_edge);
                } else if response.hovered() {
                    ui.painter().rect_filled(row, 6.0, t.hover_nav);
                }
                if response.has_focus() {
                    ui.painter().rect_stroke(row, 3.0, Stroke::new(1.5, t.focus_ring), egui::StrokeKind::Inside);
                }
                let mut end = rect.max.x - COUNT_RIGHT;
                if alias.unread > 0 {
                    let galley = ui.painter().layout_no_wrap(alias.unread.to_string(), bold(NAME_SIZE - 1.0), t.accent_soft_text);
                    let width = (galley.size().x + 12.0).max(20.0);
                    let pill = Rect::from_center_size(pos2(end + 4.0 - width / 2.0, rect.center().y), vec2(width, 18.0));
                    ui.painter().rect_filled(pill, 9.0, if active { t.surface } else { t.accent_soft });
                    ui.painter().galley(pill.center() - galley.size() / 2.0, galley, t.accent_soft_text);
                    end = pill.min.x - 6.0;
                }
                // The alias's origin colour, as its rows in combined views wear it.
                let mut start = rect.min.x + NAME_LEFT + INDENT;
                if let Some(color) = app.session.mailbox.account(account).map(|a| a.origin_row_color(Some(&alias.address))) {
                    let at = crate::unified_inbox::swatch(ui.painter(), &t, pos2(start + 5.0, rect.center().y), color);
                    app.hit.add("aliasColor", key, at);
                    start += 15.0;
                }
                paint_clipped(
                    ui.painter(),
                    pos2(start, rect.center().y),
                    (end - start).max(0.0),
                    &alias.address,
                    if active { bold(NAME_SIZE) } else { regular(NAME_SIZE) },
                    if active { t.accent_soft_text } else { t.text },
                );
                let tip = format!(
                    "{}\n{}: {}\n{}: {}",
                    alias.address,
                    tl!("Messages"),
                    alias.total,
                    tl!("Original sender unavailable"),
                    alias.missing_original
                );
                response.context_menu(|ui| crate::unified_inbox::color_menu_for(app, ui, account, Some(&alias.address)));
                if response.on_hover_text(tip).clicked() {
                    let _ = app.run("view.alias", json!({"account": account, "alias": alias.address}));
                }
            }
        },
    );
    if observed.unresolved_alias > 0 {
        let label = crate::i18n::fmt(tl!("Unresolved alias metadata: {count}"), &[("count", &observed.unresolved_alias.to_string())]);
        ui.label(egui::RichText::new(label).font(regular(NAME_SIZE)).color(t.orange))
            .on_hover_text(tl!("Missing, duplicate or invalid forwarding headers are not assigned to an alias."));
    }
}

/// What the window calls a folder: a folder of a standard kind (Inbox, Sent Items…) still called by
/// its default name, or one the server has as its folder of that kind (whatever the server calls
/// it, Borradores or Sent Mail), by the interface language's name for that kind, English too
/// (v0.12.12: every account's Drafts is Drafts), and the local folder imported mail goes to by the
/// language's word. A folder the owner named keeps its name, and the name kept in the mailbox and
/// on the server never changes.
pub(crate) fn folder_name(f: &Folder) -> &str {
    match named_by(f) {
        Some(Named::Imported) => tl!("Imported"),
        Some(Named::Kind(kind)) => crate::i18n::current().folder(kind.default_name()),
        None => &f.name,
    }
}

/// What gives a folder its name in another interface language than English ([`folder_name`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Named {
    /// The local folder imported mail goes to ([`mailcraft_model::IMPORTED`]).
    Imported,
    /// Its kind.
    Kind(FolderKind),
}

/// `None`: the folder's own name.
fn named_by(f: &Folder) -> Option<Named> {
    let on_server = !f.remote.is_empty() || f.server.is_some();
    match f.kind {
        FolderKind::Custom => (!on_server && f.name == mailcraft_model::IMPORTED).then_some(Named::Imported),
        kind => (on_server || f.name == kind.default_name()).then_some(Named::Kind(kind)),
    }
}

/// A folder's place in its account's tree (`Inbox/Projects`), each folder by [`folder_name`].
pub(crate) fn folder_path(mb: &Mailbox, id: Id) -> String {
    let mut parts = Vec::new();
    let mut cur = mb.folder(id);
    while let Some(f) = cur {
        parts.push(folder_name(f));
        // No deeper than a mailbox lets a tree go, however a file tangles it.
        if parts.len() > 32 {
            break;
        }
        cur = f.parent.and_then(|p| mb.folder(p));
    }
    parts.reverse();
    parts.join("/")
}

/// What the folder pane calls an account: its name (Account Settings › Account name), and its
/// address while none is (#1017).
pub(crate) fn account_label(a: &Account) -> String {
    if a.name.trim().is_empty() { a.email.trim().to_string() } else { shown_name(a).to_string() }
}

/// The name the program gave the account of the mail kept on this computer, before v0.12.12.
const LOCAL_NAME: &str = "On this computer";

/// An account's name as the window shows it: the account of the mail kept on this computer, still
/// called what the program named it, is the Local archive, in the interface language (v0.12.12);
/// a name the owner gave stays. The name kept in the mailbox never changes.
pub(crate) fn shown_name(a: &Account) -> &str {
    let name = a.name.trim();
    if a.kind == mailcraft_model::AccountKind::Local && a.email.trim().is_empty() && (name == LOCAL_NAME || name == "Local archive") {
        tl!("Local archive")
    } else {
        name
    }
}

/// The account as a folder row away from the pane names it: [`account_label`], and its address
/// too when the owner named it, so that two mailboxes are told apart.
pub(crate) fn account_name(a: &Account) -> String {
    let label = account_label(a);
    let email = a.email.trim();
    if email.is_empty() || email == label { label } else { format!("{label} ({email})") }
}

/// What a folder is called where it is listed away from its own account's tree (under Favorites,
/// in the bar over a search's results): its name, and for a folder of a shared mailbox the
/// mailbox it is in, so that its Inbox is never taken for the owner's (#70).
pub(crate) fn folder_label(mb: &Mailbox, f: &Folder) -> String {
    let Some(a) = mb.account(f.account).filter(|a| a.is_shared()) else { return folder_name(f).to_string() };
    let mailbox = if a.name.trim().is_empty() { a.email.trim() } else { shown_name(a) };
    crate::i18n::fmt(tl!("{folder} - {mailbox}"), &[("folder", folder_name(f)), ("mailbox", mailbox)])
}

/// A folder's name on a row of a search of every mailbox (#195): [`folder_label`], and for a
/// folder whose name a folder of another account of the owner's has too (two Inboxes), its
/// account's name after it, so the row says which one. Short otherwise: the name alone. A shared
/// mailbox's folders are named with their mailbox already, so they make no twin.
pub(crate) fn result_folder_label(mb: &Mailbox, f: &Folder) -> String {
    let shared = mb.account(f.account).is_some_and(|a| a.is_shared());
    let own = |id: Id| mb.account(id).is_some_and(|a| !a.is_shared());
    let twin = || mb.folders.iter().any(|o| o.account != f.account && own(o.account) && o.name.eq_ignore_ascii_case(&f.name));
    match mb.account(f.account) {
        Some(a) if !shared && twin() => {
            let account = if a.name.trim().is_empty() { a.email.trim() } else { shown_name(a) };
            crate::i18n::fmt(tl!("{folder} - {mailbox}"), &[("folder", folder_name(f)), ("mailbox", account)])
        }
        _ => folder_label(mb, f),
    }
}

/// What an account's heading in the folder pane says: its name, and for a shared mailbox
/// Microsoft won't open for the owner, No access (#70; the mail already here stays readable).
pub(crate) fn heading_name(app: &MailApp, a: &mailcraft_model::Account) -> String {
    if app.shared_refused.contains_key(&a.id) {
        crate::i18n::fmt(tl!("{account} · No access"), &[("account", shown_name(a))])
    } else if app.sign_in_needed.contains(&a.id) {
        crate::i18n::fmt(tl!("{account} · Sign in needed"), &[("account", shown_name(a))])
    } else {
        shown_name(a).to_string()
    }
}

#[cfg(test)]
#[path = "nav_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "nav_drop_tests.rs"]
mod drop_tests;

#[cfg(test)]
#[path = "nav_unified_tests.rs"]
mod unified_tests;

/// A folder-pane row's background: the selection (soft fill and an edge mark), the hover, and the
/// keyboard's focus ring (reached with Tab or the arrow keys; Enter or Space shows it).
fn row_backdrop(ui: &Ui, t: &Tokens, row: Rect, selected: bool, resp: &egui::Response) {
    if selected {
        ui.painter().rect_filled(row, 6.0, t.accent_soft);
        ui.painter().rect_filled(Rect::from_center_size(pos2(row.min.x + 1.5, row.center().y), vec2(3.0, 14.0)), 1.5, t.selection_edge);
    } else if resp.hovered() {
        ui.painter().rect_filled(row, 6.0, t.hover_nav);
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(row, 6.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
}
