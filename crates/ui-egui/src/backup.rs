// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backups (v0.12.9): once a day, on a thread of its own, the mailbox copied, compressed, into the
//! backup folder ([`mailcraft_engine::backup`]): a file per past year and per month of this one,
//! written again only when its mail changed. File › Options › Backups.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use mailcraft_engine::backup::{self, Job, Report};
use serde_json::{Value, json};

use crate::MailApp;

/// The first look after the window opens (not while it is busy starting), then every hour: a
/// backup is made when the last one is a day old ([`backup::FRESH`]).
const FIRST: Duration = Duration::from_secs(120);
const EVERY: Duration = Duration::from_secs(3600);

/// The backup under way, and what the last one did.
#[derive(Default)]
pub struct Backups {
    job: Option<Receiver<Result<Report, String>>>,
    next: Option<Instant>,
    pub(crate) last: Option<Report>,
    pub(crate) error: Option<String>,
}

impl Backups {
    pub fn running(&self) -> bool {
        self.job.is_some()
    }
}

/// Where backups go: the folder the owner picked, else `Backups` beside the mailbox file.
pub fn folder(app: &MailApp) -> Option<PathBuf> {
    if let Some(f) = app.ui.backup_folder.as_deref().filter(|f| !f.is_empty()) {
        return Some(PathBuf::from(f));
    }
    Some(app.session.path.as_ref()?.parent()?.join("Backups"))
}

/// Every frame: pick up a finished backup, and start the daily one when it is time.
pub(crate) fn tick(app: &mut MailApp) {
    if let Some(rx) = &app.backups.job {
        let done = match rx.try_recv() {
            Ok(done) => done,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the backup stopped before it finished".into()),
        };
        app.backups.job = None;
        match done {
            Ok(report) => {
                if !report.skipped && report.incomplete > 0 {
                    app.status(tln!(
                        report.incomplete,
                        "Backed up; {n} message is missing some content",
                        "Backed up; {n} messages are missing some content"
                    ));
                }
                app.backups.error = None;
                app.backups.last = Some(report);
            }
            Err(e) => {
                log::warn!("backup: {e}");
                app.status(tl!("The backup failed. See File › Options › Backups."));
                app.backups.error = Some(e);
            }
        }
        return;
    }
    // Only a mailbox kept in a file, once it is open.
    if !app.ui.backups || !app.autosave || app.opening.is_some() || app.session.path.is_none() {
        return;
    }
    let now = Instant::now();
    let next = *app.backups.next.get_or_insert(now + FIRST);
    if now >= next {
        app.backups.next = Some(now + EVERY);
        let _ = start(app, false);
    }
}

/// Start a backup on a thread of its own: the daily one, or (`force`) Back up now.
pub(crate) fn start(app: &mut MailApp, force: bool) -> Result<Value, String> {
    if app.backups.running() {
        return Ok(json!({"running": true}));
    }
    let (Some(path), Some(folder)) = (app.session.path.clone(), folder(app)) else {
        return Err(tl!("This mailbox isn't kept in a file, so there is nothing to back up.").to_string());
    };
    // A copy of the mailbox as it is now: its messages are shared, not copied.
    let job = Job { mailbox: app.session.mailbox.clone(), path, folder, now: app.session.now(), force };
    let (tx, rx) = channel();
    let wake = app.ctx.clone();
    std::thread::Builder::new()
        .name("backup".into())
        .spawn(move || {
            let _ = tx.send(backup::run(&job));
            if let Some(ctx) = wake {
                ctx.request_repaint();
            }
        })
        .map_err(|e| e.to_string())?;
    app.backups.job = Some(rx);
    Ok(json!({"started": true}))
}

/// `ui.backupStatus`: on or off, where, and what the last backup did.
pub(crate) fn status(app: &MailApp) -> Value {
    json!({
        "on": app.ui.backups,
        "folder": folder(app).map(|f| f.display().to_string()),
        "running": app.backups.running(),
        "last": app.backups.last,
        "error": app.backups.error,
    })
}

/// The commands: `ui.backups`, `ui.backupFolder`, `ui.backupNow`, `ui.backupStatus`.
pub(crate) fn command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        "ui.backups" => {
            let value = match p.get("value") {
                None => !app.ui.backups,
                Some(Value::Bool(value)) => *value,
                Some(_) => return Some(Err("value must be a boolean".into())),
            };
            app.ui.backups = value;
            Ok(json!({"value": value}))
        }
        "ui.backupFolder" => {
            let Some(path) = p.get("path").and_then(Value::as_str) else { return Some(Err("missing `path`".into())) };
            if !std::path::Path::new(path).is_absolute() {
                return Some(Err("the backup folder must be a full path".into()));
            }
            app.ui.backup_folder = Some(path.to_string());
            // The next backup goes there, at the next look.
            app.backups.next = None;
            Ok(json!({"folder": path}))
        }
        "ui.backupNow" => start(app, true),
        "ui.backupStatus" => Ok(status(app)),
        _ => return None,
    })
}

/// When `at` was, as the message list says it.
pub(crate) fn when(app: &MailApp, at: i64) -> String {
    crate::dates::list_label(at, app.session.now(), app.session.offset_at(at), app.session.prefs.twelve_hour)
}

/// A size as the Options page says it.
pub(crate) fn size(bytes: u64) -> String {
    mailcraft_model::size_label(bytes)
}
