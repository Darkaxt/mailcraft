// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The local archive in the window (v0.12.10): `archive.step` every minute, and the Archive
//! section of File › Options, where the owner turns it on for a Microsoft account after seeing
//! how much mail would leave the cloud ([`mailcraft_engine::cmd::archive`]).

use std::time::Duration;

use egui::Ui;
use mailcraft_model::Id;
use mailcraft_time::Stopwatch;
use serde_json::{Value, json};

use crate::MailApp;
use crate::backstage::{row as option_row, section};
use crate::theme::Tokens;

/// Seconds between two steps.
const EVERY: i64 = 60;
/// How long what the Options page shows of the archive is kept before it is read again.
const SEEN_FOR: Duration = Duration::from_secs(3);
/// The choices of years offered.
const YEARS: [u32; 5] = [1, 2, 3, 5, 10];

/// The archive's state in the window.
#[derive(Default)]
pub struct Archive {
    last: i64,
    /// Turning it on, waiting for the owner's yes: the account, the years, how many messages and
    /// their size.
    asking: Option<(Id, u32, u64, u64)>,
    seen: Option<(Stopwatch, Value)>,
}

/// Every frame: the next batch of old mail is archived, every minute.
pub(crate) fn tick(app: &mut MailApp) {
    if app.opening.is_some() || !app.session.mailbox.accounts.iter().any(|a| a.archive_after > 0) {
        return;
    }
    let now = app.session.now();
    if app.archive.last != 0 && (0..EVERY).contains(&(now - app.archive.last)) {
        return;
    }
    app.archive.last = now;
    if let Err(e) = app.execute("archive.step", json!({})) {
        log::warn!("archive: {e}");
    }
}

fn count(n: u64) -> String {
    n.to_string()
}

/// What `archive.status` says of each Microsoft account, read again after a few seconds.
fn rows(app: &mut MailApp) -> Vec<Value> {
    let fresh = app.archive.seen.as_ref().is_some_and(|(at, _)| at.so_far() < SEEN_FOR);
    if !fresh {
        let status = app.run("archive.status", json!({})).unwrap_or(Value::Null);
        app.archive.seen = Some((Stopwatch::start(), status));
    }
    app.archive.seen.as_ref().and_then(|(_, s)| s["accounts"].as_array().cloned()).unwrap_or_default()
}

/// What `archive.status` says of `account`: `None` when its old mail can't be kept here (not a
/// Microsoft account).
pub(crate) fn row_of(app: &mut MailApp, account: Id) -> Option<Value> {
    rows(app).into_iter().find(|r| r["account"].as_u64() == Some(account))
}

/// File › Options › Archive: a row for each Microsoft account.
pub(crate) fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let rows = rows(app);
    if rows.is_empty() {
        return;
    }
    section(ui, tl!("Old mail"), t);
    ui.label(egui::RichText::new(tl!("Keep old mail on this computer to free space in the cloud. It stays in the same folders.")).color(t.text_dim));
    if !app.ui.backups {
        ui.label(egui::RichText::new(tl!("Turn on backups first.")).color(t.text_dim));
    }
    for row in &rows {
        let email = row["email"].as_str().unwrap_or_default().to_string();
        control(app, ui, t, row, &email, ["optionArchive", "optionArchiveYes", "optionArchiveNo"]);
    }
}

/// The choice of years for the account of `row` (`archive.status`'), on a line that says
/// `label`, with the owner's yes before mail is moved and, once on, how far it has got. `hits`
/// name its choice, its yes and its no for the agents' map.
pub(crate) fn control(app: &mut MailApp, ui: &mut Ui, t: &Tokens, row: &Value, label: &str, hits: [&'static str; 3]) {
    let backups = app.ui.backups;
    let Some(account) = row["account"].as_u64() else { return };
    let years = row["years"].as_u64().unwrap_or(0) as u32;
    let label_of = |y: u32| if y == 0 { tl!("Off").to_string() } else { tln!(y, "Older than {n} year", "Older than {n} years") };
    let mut chosen = None;
    let mut rect = egui::Rect::NOTHING;
    option_row(ui, label, |ui| {
        ui.add_enabled_ui(backups || years > 0, |ui| {
            let combo = egui::ComboBox::from_id_salt((hits[0], account)).selected_text(label_of(years)).show_ui(ui, |ui| {
                for y in std::iter::once(0).chain(YEARS) {
                    if ui.selectable_label(y == years, label_of(y)).clicked() && y != years {
                        chosen = Some(y);
                    }
                }
            });
            rect = combo.response.rect;
        });
    });
    app.hit.add(hits[0], account, rect);
    match chosen {
        Some(0) => {
            app.archive.asking = None;
            let _ = app.run("archive.set", json!({"account": account, "years": null}));
            app.archive.seen = None;
        }
        Some(y) => {
            let p = app.run("archive.preview", json!({"account": account, "years": y})).unwrap_or(Value::Null);
            let (n, bytes) = (p["count"].as_u64().unwrap_or(0), p["bytes"].as_u64().unwrap_or(0));
            app.archive.asking = Some((account, y, n, bytes));
        }
        None => {}
    }
    if let Some((_, y, n, bytes)) = app.archive.asking.filter(|a| a.0 == account) {
        let said = crate::i18n::fmt(
            &tln!(
                n,
                "{n} email ({size}) will move to this computer and be deleted from the cloud.",
                "{n} emails ({size}) will move to this computer and be deleted from the cloud."
            ),
            &[("size", &mailcraft_model::size_label_in(crate::i18n::current(), bytes))],
        );
        ui.label(egui::RichText::new(said).color(t.text));
        ui.horizontal(|ui| {
            let yes = ui.button(tl!("Move Them"));
            app.hit.add(hits[1], account, yes.rect);
            if yes.clicked() {
                app.archive.asking = None;
                let _ = app.run("archive.set", json!({"account": account, "years": y}));
                app.archive.seen = None;
                // The first batch now, not in a minute.
                app.archive.last = 0;
            }
            let no = ui.button(tl!("Cancel"));
            app.hit.add(hits[2], account, no.rect);
            if no.clicked() {
                app.archive.asking = None;
            }
        });
    } else if years > 0 {
        let (to_move, waiting) = (row["toMove"].as_u64().unwrap_or(0), row["waiting"].as_u64().unwrap_or(0));
        let said = if to_move + waiting == 0 {
            tl!("All old mail is on this computer.").to_string()
        } else {
            crate::i18n::fmt(tl!("Moving: {left} left, {waiting} leaving the cloud"), &[("left", &count(to_move)), ("waiting", &count(waiting))])
        };
        ui.label(egui::RichText::new(said).color(t.text_dim));
    }
}
