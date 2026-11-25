// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Telling the owner when a new version of MailCraft is out, and, for a copy that updates itself,
//! installing it when the owner asks for it. Nothing is downloaded or installed by itself.
//!
//! - The daily check is off until the owner turns it on. At the first starts of a build that has
//!   it, a question above the status bar asks once a run, at most [`MOST_ASKS`] runs: Yes or No
//!   is final (File › About changes it), and a run where it is left unanswered checks nothing.
//!   A development build never asks and never checks by itself.
//! - On, it checks at most once a day, through the host ([`Services::update_check`]), which asks
//!   GitHub alone and tells it nothing of the owner. The window does no I/O of its own.
//! - The status bar has an update button. A newer version found (by the daily check, or told of in
//!   an earlier run) puts a dot on it; nothing else is said and no window opens by itself.
//! - The button (and File › About's Check for Updates) opens the update window: it checks when
//!   this run has no answer yet, then says what it found. For a newer version: what's new, and
//!   **Update now** (`ui.restartToUpdate`, the owner's click alone) on a copy that updates itself
//!   (the host gives [`Services::update_download`]: the Windows `.msi`, a Linux AppImage), unless
//!   the organisation turned it off ([`Services::updates_turned_off_by_organisation`]) or this is a
//!   development build; any other copy gets the release's page.
//! - Update now downloads the release's file (the host checks it by the release's own signature
//!   and deletes what doesn't pass), then closes the window as quitting does; the host installs
//!   the new version (Windows asks for an administrator's OK) and starts it again
//!   ([`MailApp::update_on_exit`]). Cancel while it downloads keeps MailCraft open. Quitting
//!   installs nothing, and MailCraft never restarts by itself.
//! - An agent or a script may check only while the daily check is on (`ui.checkForUpdates`), and
//!   never downloads or installs.
//! - The host says at the start what came of the last update ([`UpdateStart`]): one that wasn't
//!   installed after all is said once; a newly installed version that didn't start twice running
//!   offers to go back to the one it replaced (`ui.goBackVersion`).
//! - Kept in `ui.json`: the setting, how often it was asked, when a check last got an answer,
//!   and the last version told of ([`UiState`]); nothing else of GitHub's answer.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Ui, vec2};
use mailcraft_model::update::{Progress, Release, Version};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::widgets::primary_button;
use crate::{MailApp, Services, UiState};

/// Most starts the question is put at, unanswered, before it is never put again.
pub const MOST_ASKS: u8 = 3;
/// How long after a check that got an answer the next is due (seconds).
const DAY: i64 = 86_400;
/// How long after a check that got none the next is tried (seconds).
const AFTER_A_FAILURE: i64 = 3_600;
/// The repository the build came from, whose releases are the ones told of.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
/// The command that checks once.
pub const CHECK: &str = "ui.checkForUpdates";
/// The refusal of a check by an agent or a script while the daily check is off.
pub const NOT_ALLOWED: &str = "The owner hasn't allowed update checks.";
/// Update now: download the newer version, close the window, install it and start it.
pub const RESTART: &str = "ui.restartToUpdate";
/// The command that goes back to the version a newly installed one replaced.
pub const GO_BACK: &str = "ui.goBackVersion";
/// The refusal of either by an agent or a script.
pub const OWNERS_CLICK: &str = "Update now is the owner's click.";
/// The name `ui.dialog` opens the update window by.
pub const DIALOG: &str = "update";

/// What the host found at this start about the last update it installed (versions as text,
/// `0.5.1`; the host's own record, made safe here).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateStart {
    /// This version was to be installed when MailCraft last closed, and wasn't: the system's
    /// question (Windows' administrator prompt) was answered no, or installing failed. The old
    /// version is still here.
    pub not_installed: Option<String>,
    /// Going back to this version was asked for, and didn't happen.
    pub not_gone_back: Option<String>,
    /// The running version, newly installed, didn't start twice running, and the one it replaced
    /// is kept: (the running version, the kept one).
    pub didnt_start: Option<(String, String)>,
}

/// What the host does once the window has closed ([`MailApp::update_on_exit`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnExit {
    /// Install `version`, downloaded and checked in this run, and start it when `restart`.
    Install { version: Version, restart: bool },
    /// Put back the kept version `to` and start it.
    GoBack { to: String },
}

/// Whether MailCraft looks for new versions by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateCheck {
    /// The owner hasn't answered yet: off.
    #[default]
    NotAsked,
    On,
    Off,
}

impl UpdateCheck {
    pub fn code(self) -> &'static str {
        match self {
            UpdateCheck::NotAsked => "notAsked",
            UpdateCheck::On => "on",
            UpdateCheck::Off => "off",
        }
    }
}

/// Why Update now isn't offered on a copy that updates itself.
pub const TURNED_OFF: &str = "Turned off by your organisation";

/// Who asked for a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asker {
    /// The daily check, by itself.
    Itself,
    /// The owner: the update window, or Check for Updates.
    Owner,
    /// An agent or a script: what it finds is said, and never downloaded.
    Script,
}

/// What a run knows of new versions (not kept).
#[derive(Default)]
pub struct Updates {
    /// What is done at the start has been done.
    started: bool,
    /// The question is on screen.
    pub(crate) asking: bool,
    /// The update window is open.
    pub(crate) dialog: bool,
    /// A check on its way, and who asked for it.
    pending: Option<(Receiver<Result<Release, String>>, Asker)>,
    /// A download on its way, of that version, and how far it has got.
    downloading: Option<(Receiver<Result<PathBuf, String>>, Version, Arc<Progress>)>,
    /// The version downloaded and checked this run.
    downloaded: Option<Version>,
    /// Update now was pressed and not cancelled: once the download is checked, the window closes
    /// and the host installs it.
    installing: bool,
    /// Why the last download didn't come, for the update window.
    download_failed: Option<String>,
    /// The newest release this run found, when it is newer than the running build.
    newer: Option<Release>,
    /// A check got an answer this run: what it found stands, whatever was told before.
    answered: bool,
    /// After a check that got no answer, the next one by itself is not before this (seconds).
    retry_at: i64,
    /// What the last check the owner asked for came to, when it found nothing newer.
    said: Option<String>,
    /// The last check the owner asked for got no answer.
    failed: bool,
    /// Update now went through: the host starts the new version once it is installed.
    restart: bool,
    /// Go back was pressed.
    go_back: bool,
    /// The offer to go back was put away for this run.
    go_back_dismissed: bool,
    /// What the host said at the start has been said.
    start_said: bool,
    /// What it came to, for About: an update that wasn't installed after all.
    start_notice: Option<String>,
}

/// The version that is running.
fn running() -> Option<Version> {
    Version::parse(env!("CARGO_PKG_VERSION"))
}

/// Whether this build checks by itself and asks about it: it has a host that can ask, it is not
/// a development build, and its own version reads as one.
fn may_check_by_itself(services: &Services) -> bool {
    services.update_check.is_some() && !services.development_build && running().is_some()
}

/// The page of `version`'s release, made from its tag as this project writes them.
fn page_of(version: &Version) -> String {
    format!("{}/releases/tag/v{version}", REPOSITORY.trim_end_matches('/'))
}

/// What is said of a newer release, in the update window and in About.
fn notice(release: &Release) -> String {
    let text =
        if release.security { tl!("MailCraft {version} is available and fixes a security problem") } else { tl!("MailCraft {version} is available") };
    crate::i18n::fmt(text, &[("version", &release.version.to_string())])
}

/// Saved settings made safe to use: a count beyond the most and a version that isn't one are
/// a hostile or broken file's, not the owner's.
pub(crate) fn sanitize(ui: &mut UiState) {
    ui.update_asked = ui.update_asked.min(MOST_ASKS);
    if Version::parse(&ui.update_told).is_none() {
        ui.update_told.clear();
    }
    if Version::parse(&ui.update_skip).is_none() {
        ui.update_skip.clear();
    }
}

/// A version as the host gave it, when it is one (the host read it from a file of its own).
fn version_of(text: &str) -> Option<String> {
    Version::parse(text).map(|v| v.to_string())
}

impl MailApp {
    /// The newest release known of that is newer than the running build: found this run, or
    /// told of in an earlier one.
    pub(crate) fn newer_release(&self) -> Option<Release> {
        let running = running()?;
        if self.updates.answered {
            return self.updates.newer.clone();
        }
        let told = Version::parse(&self.ui.update_told).filter(|told| told.is_newer_than(&running))?;
        Some(Release { page: page_of(&told), version: told, security: false, assets: Vec::new() })
    }

    /// Once, at the start of a run (after the settings are read): put the question, when it is
    /// still to be put.
    fn updates_at_start(&mut self) {
        if std::mem::replace(&mut self.updates.started, true) {
            return;
        }
        if may_check_by_itself(&self.services) && self.ui.update_check == UpdateCheck::NotAsked && self.ui.update_asked < MOST_ASKS {
            self.ui.update_asked = self.ui.update_asked.saturating_add(1);
            self.updates.asking = true;
        }
        if !std::mem::replace(&mut self.updates.start_said, true) {
            let start = &self.services.update_start;
            let said = if let Some(version) = start.not_installed.as_deref().and_then(version_of) {
                Some(crate::i18n::fmt(tl!("MailCraft {version} wasn't installed: this version is still the one here."), &[("version", &version)]))
            } else {
                start.not_gone_back.as_deref().and_then(version_of).map(|version| {
                    crate::i18n::fmt(
                        tl!("Going back to MailCraft {version} didn't happen: this version is still the one here."),
                        &[("version", &version)],
                    )
                })
            };
            if let Some(said) = said {
                self.status(said.clone());
                self.updates.start_notice = Some(said);
            }
        }
    }

    /// The version downloaded and checked in this run.
    pub(crate) fn update_ready(&self) -> Option<&Version> {
        self.updates.downloaded.as_ref()
    }

    /// The offer to go back, while it stands: (the running version, the kept one).
    fn go_back_offer(&self) -> Option<(String, String)> {
        let (this, kept) = self.services.update_start.didnt_start.as_ref()?;
        let both = version_of(this).zip(version_of(kept));
        both.filter(|_| !self.updates.go_back_dismissed && !self.updates.go_back)
    }

    /// For the host, once the window has closed (after the mailbox and the settings are saved):
    /// go back to the kept version, when the owner pressed Go back; else install the version
    /// downloaded and checked in this run and start it, when the owner pressed Update now and it
    /// went through. `None`: nothing to do (quitting installs nothing). The host installs only
    /// what it checked itself.
    pub fn update_on_exit(&self) -> Option<OnExit> {
        if self.updates.go_back
            && let Some((_, kept)) = self.services.update_start.didnt_start.as_ref()
            && let Some(to) = version_of(kept)
        {
            return Some(OnExit::GoBack { to });
        }
        if !self.updates.restart {
            return None;
        }
        let version = self.update_ready()?.clone();
        Some(OnExit::Install { version, restart: true })
    }

    /// Whether this copy offers Update now: it updates itself, the organisation hasn't turned it
    /// off, and this is not a development build.
    pub(crate) fn installs_updates(&self) -> bool {
        install_setting(self) == "on"
    }

    /// `ui.dialog {"name": "update"}`: the update window. It checks when this run has no answer
    /// yet (and none is on its way).
    pub(crate) fn open_update_dialog(&mut self) -> Result<Value, String> {
        if self.services.update_check.is_none() {
            return Err(tl!("This build doesn't check for new versions.").to_string());
        }
        self.updates.dialog = true;
        if !self.updates.answered && self.updates.pending.is_none() {
            self.begin_update_check(Asker::Owner);
        }
        Ok(json!({"dialog": DIALOG}))
    }

    /// `ui.restartToUpdate`, Update now, the owner's click: download the newer version this run
    /// found, then close as quitting does; the host installs it and starts it.
    pub(crate) fn restart_to_update(&mut self) -> Result<Value, String> {
        if !self.installs_updates() {
            return Err(tl!("This copy of MailCraft doesn't update itself.").to_string());
        }
        let Some(release) = self.updates.newer.clone() else {
            return Err(tl!("No new version is ready to install.").to_string());
        };
        let version = release.version.to_string();
        self.updates.download_failed = None;
        if self.updates.downloaded.as_ref() == Some(&release.version) {
            self.install_now();
            return Ok(json!({"pending": "restart", "version": version}));
        }
        self.updates.installing = true;
        self.begin_update_download(&release);
        if self.updates.downloading.is_none() {
            self.updates.installing = false;
            return Err(tl!("This copy of MailCraft doesn't update itself.").to_string());
        }
        Ok(json!({"pending": "download", "version": version}))
    }

    /// The download is checked: close as quitting does, and the host installs it.
    fn install_now(&mut self) {
        self.updates.installing = false;
        self.updates.restart = true;
        self.updates.dialog = false;
        self.quit_requested = true;
    }

    /// `ui.goBackVersion`, the owner's click: close as quitting does, then the host puts the kept
    /// version back and starts it.
    pub(crate) fn go_back_a_version(&mut self) -> Result<Value, String> {
        let Some((this, kept)) = self.go_back_offer() else {
            return Err(tl!("There is no earlier version to go back to.").to_string());
        };
        self.updates.go_back = true;
        self.ui.update_skip = this;
        self.quit_requested = true;
        Ok(json!({"pending": "goBack", "version": kept}))
    }

    /// `ui.dismissUpdateNotice`.
    pub(crate) fn dismiss_update_notice(&mut self, p: &Value) -> Result<Value, String> {
        match p.get("notice").and_then(Value::as_str) {
            Some("didntStart") => self.updates.go_back_dismissed = true,
            _ => return Err(tl!("`notice` is didntStart").to_string()),
        }
        Ok(update_state(self))
    }

    /// Every frame: the start's question, a check that is due, and the answer to one.
    pub(crate) fn updates_tick(&mut self, ctx: &egui::Context) {
        self.updates_at_start();
        self.poll_update_check();
        self.poll_update_download();
        // (Working offline, nothing is checked by itself: the check is due again once it is off.)
        if self.ui.update_check != UpdateCheck::On || !may_check_by_itself(&self.services) || self.updates.pending.is_some() || self.offline() {
            return;
        }
        let now = self.session.now();
        let due = self.ui.update_checked.saturating_add(DAY).max(self.updates.retry_at);
        if now >= due {
            self.begin_update_check(Asker::Itself);
        } else {
            // Looked at again when it is due (an hour at a time: a computer that slept wakes up late).
            let wait = u64::try_from(due.saturating_sub(now).clamp(1, AFTER_A_FAILURE)).unwrap_or(1);
            ctx.request_repaint_after(std::time::Duration::from_secs(wait));
        }
    }

    /// Hand a check to the host.
    fn begin_update_check(&mut self, asker: Asker) -> bool {
        let Some(check) = &self.services.update_check else { return false };
        let pre_releases = running().is_some_and(|v| v.is_pre_release());
        self.updates.pending = Some((check(pre_releases), asker));
        if asker == Asker::Owner {
            self.updates.said = None;
            self.updates.failed = false;
        }
        true
    }

    /// Hand the download of `release` to the host, when it is newer and isn't on its way already.
    fn begin_update_download(&mut self, release: &Release) {
        let wanted = running().is_some_and(|running| release.version.is_newer_than(&running))
            && self.updates.downloading.as_ref().is_none_or(|(_, v, _)| *v != release.version);
        let Some(download) = self.services.update_download.as_ref().filter(|_| wanted) else { return };
        let progress = Arc::new(Progress::default());
        self.updates.downloading = Some((download(release.clone(), progress.clone()), release.version.clone(), progress));
    }

    fn poll_update_download(&mut self) {
        let Some((rx, version, _)) = &self.updates.downloading else { return };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the download stopped unexpectedly".to_string()),
        };
        let version = version.clone();
        self.updates.downloading = None;
        // One line in the log, naming no address (the host's words name none).
        match answer {
            Ok(_) => {
                log::info!("MailCraft {version} is downloaded and checked by its release signature");
                self.updates.downloaded = Some(version);
                if self.updates.installing {
                    self.install_now();
                }
            }
            Err(why) => {
                log::info!("MailCraft {version} wasn't downloaded: {why}");
                if std::mem::take(&mut self.updates.installing) {
                    self.updates.download_failed = Some(why);
                }
            }
        }
    }

    fn poll_update_check(&mut self) {
        let Some((rx, asker)) = &self.updates.pending else { return };
        let asker = *asker;
        let pressed = asker == Asker::Owner;
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the check stopped unexpectedly".to_string()),
        };
        self.updates.pending = None;
        let now = self.session.now();
        match answer {
            Ok(release) => {
                self.ui.update_checked = now;
                self.updates.answered = true;
                if running().is_some_and(|running| release.version.is_newer_than(&running)) {
                    // The dot on the update button says it; nothing else, unless asked.
                    self.ui.update_told = release.version.to_string();
                    if asker == Asker::Script {
                        self.status(notice(&release));
                    }
                    self.updates.newer = Some(release);
                } else {
                    self.updates.newer = None;
                    if pressed {
                        self.updates.said =
                            Some(crate::i18n::fmt(tl!("MailCraft {version} is up to date."), &[("version", env!("CARGO_PKG_VERSION"))]));
                    }
                }
            }
            Err(why) => {
                // (The host's words name no address.)
                log::info!("couldn't find out whether a new version of MailCraft is out: {why}");
                self.updates.retry_at = now.saturating_add(AFTER_A_FAILURE);
                if pressed {
                    self.updates.failed = true;
                    self.updates.said = Some(why);
                }
            }
        }
    }

    /// `ui.checkForUpdates`: check once. `pressed`: the owner pressed Check for Updates, which opens
    /// the update window; else an agent or a script asks, which only the daily check being on
    /// allows.
    pub(crate) fn check_for_updates(&mut self, pressed: bool) -> Result<Value, String> {
        if pressed {
            self.updates.answered = false;
            if self.updates.pending.is_none() && !self.begin_update_check(Asker::Owner) {
                return Err(tl!("This build doesn't check for new versions.").to_string());
            }
            self.updates.dialog = true;
            return Ok(json!({"pending": "updateCheck", "version": env!("CARGO_PKG_VERSION")}));
        }
        if self.ui.update_check != UpdateCheck::On {
            return Err(tl!(NOT_ALLOWED).to_string());
        }
        if self.updates.pending.is_none() && !self.begin_update_check(Asker::Script) {
            return Err(tl!("This build doesn't check for new versions.").to_string());
        }
        Ok(json!({"pending": "updateCheck", "version": env!("CARGO_PKG_VERSION")}))
    }

    /// `ui.updateCheck`: the daily check on or off (the question's answer, File › About).
    pub(crate) fn set_update_check(&mut self, p: &Value) -> Result<Value, String> {
        let on = match p.get("value") {
            None | Some(Value::Null) => self.ui.update_check != UpdateCheck::On,
            Some(value) => value.as_bool().ok_or_else(|| tl!("`value` is true or false").to_string())?,
        };
        self.ui.update_check = if on { UpdateCheck::On } else { UpdateCheck::Off };
        self.updates.asking = false;
        Ok(update_state(self))
    }
}

/// Whether Update now is offered: `on` (a copy that updates itself), `organisation` (turned off
/// for every user of the computer), or `notThisCopy` (a copy that doesn't update itself, or a
/// development build).
fn install_setting(app: &MailApp) -> &'static str {
    if app.services.update_download.is_none() || app.services.development_build || !may_check_by_itself(&app.services) {
        "notThisCopy"
    } else if app.services.updates_turned_off_by_organisation {
        "organisation"
    } else {
        "on"
    }
}

/// What `ui.updateCheck` answers: the setting, and what is known.
fn update_state(app: &MailApp) -> Value {
    let newer = app.newer_release();
    json!({
        "install": {
            "value": app.installs_updates(),
            "setting": install_setting(app),
            "downloading": app.updates.downloading.is_some(),
            "progress": app.updates.downloading.as_ref().map(|(_, _, p)| json!({"received": p.received(), "total": p.total()})),
            "downloaded": app.updates.downloaded.as_ref().map(ToString::to_string),
            "installing": app.updates.installing,
            "restart": app.updates.restart,
            "didntStart": app.go_back_offer().map(|(this, kept)| json!({"version": this, "previous": kept})),
        },
        "value": app.ui.update_check == UpdateCheck::On,
        "setting": app.ui.update_check.code(),
        "asked": app.ui.update_asked,
        "version": env!("CARGO_PKG_VERSION"),
        "newer": newer.as_ref().map(|r| json!({"version": r.version.to_string(), "security": r.security, "page": r.page})),
        "checking": app.updates.pending.is_some(),
        "dialog": app.updates.dialog,
    })
}

/// What is put across the window above the status bar, one at a time: the offer to go back to
/// the version before, or the question about checking for new versions.
pub(crate) fn question(app: &mut MailApp, ui: &mut Ui) {
    if let Some((this, kept)) = app.go_back_offer() {
        let said = crate::i18n::fmt(tl!("MailCraft {version} didn't start."), &[("version", &this)]);
        let back = crate::i18n::fmt(tl!("Go back to version {version}"), &[("version", &kept)]);
        bar(app, ui, "update_go_back", &said, &[("goBack", &back, true), ("keepVersion", tl!("Keep this version"), false)], |app, which| {
            let _ = if which == "goBack" { app.run(GO_BACK, json!({})) } else { app.run("ui.dismissUpdateNotice", json!({"notice": "didntStart"})) };
        });
    } else if app.updates.asking {
        let said = tl!("Check daily for new versions of MailCraft? Only GitHub is asked; nothing about you or your mail is sent.");
        bar(app, ui, "update_question", said, &[("updateYes", tl!("Yes"), true), ("updateNo", tl!("No"), false)], |app, which| {
            let _ = app.run("ui.updateCheck", json!({"value": which == "updateYes"}));
        });
    }
}

/// A line across the window above the status bar: `said`, then its buttons (hit name, label,
/// whether it is the primary one); `pressed` gets the hit name of the one pressed.
pub(crate) fn bar(
    app: &mut MailApp,
    ui: &mut Ui,
    id: &'static str,
    said: &str,
    buttons: &[(&'static str, &str, bool)],
    pressed: impl FnOnce(&mut MailApp, &str),
) {
    let t = Tokens::get(ui.ctx());
    let mut clicked = None;
    egui::Panel::bottom(id)
        .exact_size(40.0)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 0,
            bottom: 0,
        }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                crate::icons::paint(
                    ui.painter(),
                    egui::Rect::from_min_size(ui.cursor().min + vec2(0.0, 11.0), vec2(18.0, 18.0)),
                    "info",
                    t.icon,
                    t.accent,
                );
                ui.add_space(24.0);
                // A line too long for the window (or longer in another language) is cut short
                // and whole on hover, so the buttons stay on screen.
                let room = (ui.available_width() - buttons.len() as f32 * 84.0).max(80.0);
                let font = egui::TextStyle::Body.resolve(ui.style());
                let wide = ui.painter().layout_no_wrap(said.to_string(), font, t.text).size().x;
                let text = egui::RichText::new(said).color(t.text);
                if wide <= room {
                    ui.label(text);
                } else {
                    ui.scope(|ui| {
                        ui.set_max_width(room);
                        ui.add(egui::Label::new(text).truncate()).on_hover_text(said);
                    });
                }
                for (name, label, primary) in buttons {
                    let button =
                        if *primary { primary_button(ui, label, true) } else { ui.add(egui::Button::new(*label).min_size(vec2(64.0, 26.0))) };
                    app.hit.add(name, 0, button.rect);
                    if button.clicked() {
                        clicked = Some(*name);
                    }
                }
            });
        });
    if let Some(name) = clicked {
        pressed(app, name);
    }
}

/// The status bar, laid out right to left: the update button, with a dot while a newer version
/// is known. A click opens the update window.
pub(crate) fn status_item(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    if app.services.update_check.is_none() {
        return;
    }
    let newer = app.newer_release();
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 20.0), egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }
    let glyph = egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
    crate::icons::paint(ui.painter(), glyph, "reset", if hovered { t.text } else { t.text_dim }, t.accent);
    if newer.is_some() {
        let dot = egui::pos2(rect.right() - 4.0, rect.top() + 4.0);
        ui.painter().circle_filled(dot, 4.0, t.status_bar);
        ui.painter().circle_filled(dot, 3.0, t.accent);
    }
    app.hit.add("updateButton", 0, rect);
    let tip = match &newer {
        Some(release) => crate::i18n::fmt(
            tl!("MailCraft {version} is available: click to see what's new and update"),
            &[("version", &release.version.to_string())],
        ),
        None => tl!("Check for updates").to_string(),
    };
    let label = tip.clone();
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    ui.add_space(8.0);
    if response.clicked() {
        let _ = app.run("ui.dialog", json!({"name": DIALOG}));
    }
}

/// The update window, while it is open: checking, what a check found, the download.
pub(crate) fn dialog(app: &mut MailApp, ctx: &egui::Context) {
    if !app.updates.dialog {
        return;
    }
    let t = Tokens::get(ctx);
    let here = env!("CARGO_PKG_VERSION");
    let mut pressed: Option<&'static str> = None;
    let checking = app.updates.pending.is_some();
    let newer = app.updates.answered.then(|| app.updates.newer.clone()).flatten();
    let downloading = app.updates.installing;
    let progress = app.updates.downloading.as_ref().map(|(_, _, p)| (p.received(), p.total()));
    let can_install = app.installs_updates();
    let open = crate::dialogs::window(ctx, tl!("Update"), 420.0, |ui| {
        let heading = |ui: &mut Ui, text: &str, color: egui::Color32| {
            ui.label(egui::RichText::new(text).font(crate::theme::semibold(15.0)).color(color));
        };
        let dim = |ui: &mut Ui, text: &str| {
            ui.label(egui::RichText::new(text).color(t.text_dim));
        };
        let you_have = crate::i18n::fmt(tl!("You have MailCraft {version}."), &[("version", here)]);
        if checking {
            heading(ui, tl!("Checking for updates…"), t.text);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(14.0));
                dim(ui, &you_have);
            });
            crate::dialogs::button_row(ui, |ui| {
                if ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked() {
                    pressed = Some("close");
                }
            });
        } else if let Some(release) = newer {
            if downloading {
                heading(ui, &crate::i18n::fmt(tl!("Updating to MailCraft {version}"), &[("version", &release.version.to_string())]), t.text);
                dim(ui, tl!("MailCraft closes, installs it and opens again. Windows asks for your OK first."));
                match progress.filter(|(_, total)| *total > 0) {
                    // How much is there and how much there is in all, on a bar as opening the mailbox has.
                    Some((got, total)) => {
                        let got = got.min(total);
                        ui.add_space(4.0);
                        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), egui::Sense::hover());
                        ui.painter().rect_filled(bar, 3.0, t.track);
                        let fraction = (got as f64 / total as f64) as f32;
                        if fraction > 0.0 {
                            let filled = egui::Rect::from_min_max(bar.min, egui::pos2(bar.min.x + (bar.width() * fraction).max(6.0), bar.max.y));
                            ui.painter().rect_filled(filled, 3.0, t.accent);
                        }
                        app.hit.add("updateProgress", 0, bar);
                        let said = crate::i18n::fmt(
                            tl!("{read} of {total} MB"),
                            &[("read", &crate::opening::megabytes(got, total)), ("total", &crate::opening::megabytes(total, total))],
                        );
                        dim(ui, &said);
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(14.0));
                            ui.label(tl!("Downloading…"));
                        });
                    }
                }
                // The bar moves as the bytes come, not only when something else wakes the window.
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                crate::dialogs::button_row(ui, |ui| {
                    let cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0)));
                    app.hit.add("updateCancel", 0, cancel.rect);
                    if cancel.clicked() {
                        pressed = Some("cancel");
                    }
                });
            } else {
                heading(ui, &notice(&release), if release.security { t.red } else { t.text });
                dim(ui, &you_have);
                let page = release.page.clone();
                if ui.link(tl!("What's new")).on_hover_text(&page).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(page.clone()));
                }
                if let Some(why) = app.updates.download_failed.clone() {
                    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Couldn't download it: {why}."), &[("why", &why)])).color(t.red));
                } else if app.services.updates_turned_off_by_organisation {
                    dim(ui, tl!(TURNED_OFF));
                }
                ui.add_space(4.0);
                crate::dialogs::button_row(ui, |ui| {
                    if can_install {
                        let label = if app.updates.download_failed.is_some() { tl!("Try again") } else { tl!("Update now") };
                        let update = primary_button(ui, label, true);
                        app.hit.add("updateNow", 0, update.rect);
                        if update.clicked() {
                            pressed = Some("update");
                        }
                    } else {
                        let get = primary_button(ui, tl!("Open download page"), true);
                        app.hit.add("updatePage", 0, get.rect);
                        if get.clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(page.clone()));
                        }
                    }
                    let later = ui.add(egui::Button::new(tl!("Later")).min_size(vec2(72.0, 26.0)));
                    app.hit.add("updateLater", 0, later.rect);
                    if later.clicked() {
                        pressed = Some("close");
                    }
                });
            }
        } else {
            let failed = app.updates.failed;
            if failed {
                heading(ui, tl!("Couldn't check for updates"), t.text);
                dim(ui, app.updates.said.as_deref().unwrap_or_default());
            } else {
                heading(ui, &crate::i18n::fmt(tl!("MailCraft {version} is up to date"), &[("version", here)]), t.text);
            }
            crate::dialogs::button_row(ui, |ui| {
                let again = ui.add(egui::Button::new(if failed { tl!("Try again") } else { tl!("Check again") }).min_size(vec2(72.0, 26.0)));
                app.hit.add("updateCheckAgain", 0, again.rect);
                if again.clicked() {
                    pressed = Some("again");
                }
                if ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked() {
                    pressed = Some("close");
                }
            });
        }
    });
    match pressed {
        Some("update") => {
            let _ = app.run(RESTART, json!({}));
        }
        Some("again") => {
            let _ = app.run(CHECK, json!({}));
        }
        Some("cancel") => app.updates.installing = false,
        Some(_) => app.updates.dialog = false,
        None if !open => {
            app.updates.installing = false;
            app.updates.dialog = false;
        }
        None => {}
    }
}

/// File › About: the newer version, when there is one, Check for Updates and the daily switch.
pub(crate) fn about(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    if app.services.update_check.is_none() {
        return;
    }
    if let Some(release) = app.newer_release() {
        let color = if release.security { t.red } else { t.accent_text };
        ui.label(egui::RichText::new(notice(&release)).font(crate::theme::semibold(13.0)).color(color));
    }
    if let Some(said) = app.updates.start_notice.clone() {
        ui.label(egui::RichText::new(said).color(t.text_dim));
    }
    let checking = app.updates.pending.is_some();
    let label = if checking { tl!("Checking…") } else { tl!("Check for Updates") };
    let pressed = ui.add_enabled(!checking, egui::Button::new(label));
    app.hit.add("checkForUpdates", 0, pressed.rect);
    if pressed.clicked() {
        let _ = app.run(CHECK, json!({}));
    }
    ui.add_space(8.0);
    let mut on = app.ui.update_check == UpdateCheck::On;
    let switch = ui.checkbox(&mut on, tl!("Check for new versions"));
    app.hit.add("updateOption", 0, switch.rect);
    if switch.changed() {
        let _ = app.run("ui.updateCheck", json!({"value": on}));
    }
    let detail = if app.services.development_build {
        tl!("This is a development build: it checks only when you press Check for Updates.")
    } else if install_setting(app) == "on" {
        tl!("Once a day. Only GitHub is asked; nothing about you or your mail is sent. Nothing is installed until you click Update now.")
    } else {
        tl!("Once a day. Only GitHub is asked; nothing about you or your mail is sent. Nothing is downloaded or installed.")
    };
    ui.label(egui::RichText::new(detail).color(t.text_dim));
}

/// The window commands about new versions (`UI_COMMANDS` lists them). `ui.checkForUpdates` here
/// is a script's or an agent's; the owner's press goes through [`MailApp::run`].
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    Some(match id {
        CHECK => app.check_for_updates(false),
        "ui.updateCheck" => app.set_update_check(p),
        "ui.dismissUpdateNotice" => app.dismiss_update_notice(p),
        // The owner's clicks alone ([`MailApp::run`]).
        RESTART | GO_BACK => Err(tl!(OWNERS_CLICK).to_string()),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use mailcraft_engine::Session;

    use super::*;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    /// A stand-in host: every check answers what `answer` holds, and is counted.
    struct Host {
        answer: Arc<Mutex<Result<Release, String>>>,
        asked: Arc<AtomicUsize>,
    }

    fn release(version: &str, security: bool) -> Release {
        let version = Version::parse(version).unwrap();
        Release { page: page_of(&version), version, security, assets: Vec::new() }
    }

    fn stand_in(answer: Result<Release, String>) -> (Host, Services) {
        let host = Host { answer: Arc::new(Mutex::new(answer)), asked: Arc::new(AtomicUsize::new(0)) };
        let (answer, asked) = (host.answer.clone(), host.asked.clone());
        let services = Services {
            update_check: Some(Box::new(move |_| {
                asked.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = std::sync::mpsc::channel();
                let _ = tx.send(answer.lock().unwrap().clone());
                rx
            })),
            ..Default::default()
        };
        (host, services)
    }

    /// A start of the app at `now`, with the settings `saved` from the run before.
    fn start(services: Services, saved: &UiState, now: i64) -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), now, 0), services);
        app.autosave = false;
        app.apply_prefs(saved.clone());
        app
    }

    fn frame(app: &mut MailApp) {
        app.updates_tick(&egui::Context::default());
    }

    fn said(app: &MailApp) -> Option<String> {
        app.status_msg.as_ref().map(|(text, _)| text.clone())
    }

    fn newer() -> String {
        let running = running().unwrap();
        format!("{}.{}.{}", running.major, running.minor.saturating_add(1), 0)
    }

    fn on() -> UiState {
        UiState { update_check: UpdateCheck::On, ..Default::default() }
    }

    /// `services` with a stand-in download that answers `answer` and counts what it was asked for.
    fn downloading(mut services: Services, answer: Result<PathBuf, String>) -> (Services, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let kept = asked.clone();
        services.update_download = Some(Box::new(move |release: Release, _| {
            kept.lock().unwrap().push(release.version.to_string());
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(answer.clone());
            rx
        }));
        (services, asked)
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn the_question_is_put_at_three_starts_at_most() {
        let mut saved = UiState::default();
        for start_number in 1..=5u8 {
            let (host, services) = stand_in(Ok(release(&newer(), false)));
            let mut app = start(services, &saved, NOW);
            frame(&mut app);
            assert_eq!(app.updates.asking, start_number <= MOST_ASKS, "start {start_number}");
            // Unanswered: nothing is checked in that run.
            for _ in 0..3 {
                frame(&mut app);
            }
            assert_eq!(host.asked.load(Ordering::Relaxed), 0);
            assert_eq!(app.ui.update_check, UpdateCheck::NotAsked);
            saved = app.prefs();
        }
        assert_eq!(saved.update_asked, MOST_ASKS);
    }

    #[test]
    fn an_answer_is_final() {
        // No: never asked again, nothing checked.
        let (host_no, services) = stand_in(Ok(release(&newer(), false)));
        let mut app = start(services, &UiState::default(), NOW);
        frame(&mut app);
        assert!(app.updates.asking);
        assert_eq!(app.run("ui.updateCheck", json!({"value": false})).unwrap()["setting"], "off");
        assert!(!app.updates.asking);
        frame(&mut app);
        let saved = app.prefs();
        let (_, services) = stand_in(Ok(release(&newer(), false)));
        let mut again = start(services, &saved, NOW + 2 * DAY);
        frame(&mut again);
        assert!(!again.updates.asking && again.ui.update_check == UpdateCheck::Off);
        assert_eq!(host_no.asked.load(Ordering::Relaxed), 0);

        // Yes: checked at once, and not asked again either.
        let (host_yes, services) = stand_in(Ok(release(&newer(), false)));
        let mut app = start(services, &UiState::default(), NOW);
        frame(&mut app);
        assert_eq!(app.run("ui.updateCheck", json!({"value": true})).unwrap()["value"], true);
        frame(&mut app);
        frame(&mut app);
        assert_eq!(host_yes.asked.load(Ordering::Relaxed), 1);
        let (_, services) = stand_in(Ok(release(&newer(), false)));
        let mut again = start(services, &app.prefs(), NOW + 60);
        frame(&mut again);
        assert!(!again.updates.asking);
    }

    #[test]
    fn the_daily_check_only_lights_the_dot() {
        let (host, services) = stand_in(Ok(release(&newer(), false)));
        let (services, downloads) = downloading(services, Ok(PathBuf::from("/updates/x")));
        let mut saved = on();
        let mut app = start(services, &saved, NOW);
        for _ in 0..3 {
            frame(&mut app);
        }
        assert_eq!(host.asked.load(Ordering::Relaxed), 1);
        // Nothing said, no window, nothing downloaded: the newer release is known, for the dot.
        assert_eq!(said(&app), None);
        assert!(!app.updates.dialog);
        assert!(downloads.lock().unwrap().is_empty());
        assert_eq!(app.ui.update_told, newer());
        assert_eq!(app.ui.update_checked, NOW);
        assert_eq!(app.newer_release().unwrap().page, format!("https://github.com/Darkaxt/mailcraft/releases/tag/v{}", newer()));
        assert!(app.update_on_exit().is_none(), "quitting installs nothing");
        saved = app.prefs();

        // Within the day: nobody is asked, and the dot stays from what was kept.
        let (host, services) = stand_in(Ok(release(&newer(), false)));
        let mut app = start(services, &saved, NOW + DAY - 1);
        frame(&mut app);
        assert_eq!(host.asked.load(Ordering::Relaxed), 0);
        assert_eq!(app.newer_release().map(|r| r.version.to_string()), Some(newer()));

        // A day on: asked again.
        let (host, services) = stand_in(Ok(release(&newer(), false)));
        let mut app = start(services, &saved, NOW + DAY);
        frame(&mut app);
        frame(&mut app);
        assert_eq!(host.asked.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_update_window_checks_and_says_what_it_found() {
        let here = env!("CARGO_PKG_VERSION");
        let (host, services) = stand_in(Ok(release(here, false)));
        let mut app = start(services, &UiState::default(), NOW);
        assert_eq!(app.run("ui.dialog", json!({"name": DIALOG})).unwrap()["dialog"], DIALOG);
        assert!(app.updates.dialog);
        assert_eq!(host.asked.load(Ordering::Relaxed), 1, "checked even with the daily check off");
        frame(&mut app);
        assert_eq!(app.updates.said, Some(format!("MailCraft {here} is up to date.")));
        // Opened again in the same run: the answer stands, nobody is asked.
        app.updates.dialog = false;
        app.run("ui.dialog", json!({"name": DIALOG})).unwrap();
        assert_eq!(host.asked.load(Ordering::Relaxed), 1);
        // Check again (Check for Updates): asked again.
        app.run(CHECK, json!({})).unwrap();
        assert_eq!(host.asked.load(Ordering::Relaxed), 2);
        frame(&mut app);
        // A failure is said in the window.
        *host.answer.lock().unwrap() = Err("GitHub couldn't be reached".into());
        app.run(CHECK, json!({})).unwrap();
        frame(&mut app);
        assert!(app.updates.failed);
        assert_eq!(app.updates.said.as_deref(), Some("GitHub couldn't be reached"));
        // An older release is nothing newer.
        *host.answer.lock().unwrap() = Ok(release("0.0.1", false));
        app.run(CHECK, json!({})).unwrap();
        frame(&mut app);
        assert!(app.newer_release().is_none() && !app.updates.failed);
        // What was told in an earlier run doesn't outlast an answer that has nothing newer.
        app.ui.update_told = newer();
        assert!(app.newer_release().is_none());
        // A build without a host has no window.
        let mut bare = start(Services::default(), &UiState::default(), NOW);
        assert!(bare.run("ui.dialog", json!({"name": DIALOG})).is_err());
    }

    #[test]
    fn development_builds_never_ask_or_check_by_themselves() {
        let (host, mut services) = stand_in(Ok(release(&newer(), false)));
        services.development_build = true;
        let (services, _) = downloading(services, Ok(PathBuf::new()));
        let mut app = start(services, &UiState::default(), NOW);
        frame(&mut app);
        assert!(!app.updates.asking);
        app.ui.update_check = UpdateCheck::On;
        frame(&mut app);
        frame(&mut app);
        assert_eq!(host.asked.load(Ordering::Relaxed), 0);
        // Check for Updates still works, and offers no Update now.
        app.run(CHECK, json!({})).unwrap();
        frame(&mut app);
        assert_eq!(host.asked.load(Ordering::Relaxed), 1);
        assert!(app.newer_release().is_some());
        assert!(!app.installs_updates());
        assert!(app.run(RESTART, json!({})).is_err());
    }

    #[test]
    fn agents_and_scripts_check_only_while_the_owner_has_it_on() {
        let (host, services) = stand_in(Ok(release(&newer(), false)));
        let (services, downloads) = downloading(services, Ok(PathBuf::new()));
        let mut app = start(services, &UiState { update_check: UpdateCheck::Off, ..Default::default() }, NOW);
        frame(&mut app);
        assert_eq!(app.execute(CHECK, json!({})), Err(NOT_ALLOWED.to_string()));
        app.ui.update_check = UpdateCheck::NotAsked;
        assert_eq!(app.execute(CHECK, json!({})), Err(NOT_ALLOWED.to_string()));
        assert_eq!(host.asked.load(Ordering::Relaxed), 0);
        // Through the agents' gate as the MCP bridge sends it: refused the same way.
        let (request, _answer) = crate::ControlRequest::new("engine.execute", json!({"command": CHECK, "params": {}}));
        let request = request.from_agent(Vec::new());
        let Some(crate::control::Outcome::Done(reply)) = crate::agent_gate::screen(&mut app, &request, &[]) else { panic!("settled by the gate") };
        assert_eq!(reply["ok"], false, "{reply}");
        assert!(reply["error"].as_str().unwrap().contains(NOT_ALLOWED), "{reply}");
        assert_eq!(host.asked.load(Ordering::Relaxed), 0);
        // On: an agent may, what it finds is said, and nothing is downloaded or opened.
        app.ui.update_check = UpdateCheck::On;
        assert_eq!(app.execute(CHECK, json!({})).unwrap()["pending"], "updateCheck");
        frame(&mut app);
        assert_eq!(host.asked.load(Ordering::Relaxed), 1);
        assert_eq!(said(&app), Some(format!("MailCraft {} is available", newer())));
        assert!(!app.updates.dialog);
        assert!(downloads.lock().unwrap().is_empty());
        // This reversible preference needs no blanket settings prompt.
        assert_eq!(mailcraft_engine::agent::access("ui.updateCheck"), Some(mailcraft_engine::agent::Access::Safe));
        let mut bare = start(Services::default(), &UiState::default(), NOW);
        assert!(bare.run(CHECK, json!({})).is_err());
        assert!(bare.execute("ui.updateCheck", json!({"value": "yes"})).is_err());
    }

    #[test]
    fn what_was_kept_is_made_safe() {
        let saved = UiState { update_asked: 250, update_told: "9.9.9\r\n<script>".into(), update_checked: i64::MAX, ..Default::default() };
        let (host, services) = stand_in(Ok(release(&newer(), false)));
        let mut app = start(services, &saved, NOW);
        assert_eq!(app.ui.update_asked, MOST_ASKS);
        assert!(app.ui.update_told.is_empty());
        frame(&mut app);
        assert!(!app.updates.asking);
        // A last check in the far future doesn't overflow anything.
        app.ui.update_check = UpdateCheck::On;
        frame(&mut app);
        assert_eq!(host.asked.load(Ordering::Relaxed), 0);
        let json: UiState = serde_json::from_value(json!({"updateCheck": "sometimes", "updateAsked": -1})).unwrap_or_default();
        assert_eq!(json.update_check, UpdateCheck::NotAsked);
    }

    /// A copy that updates itself whose newer release this run's check found, after a few frames.
    fn found_app(security: bool, update_start: UpdateStart, download: Result<PathBuf, String>) -> (MailApp, Arc<Mutex<Vec<String>>>) {
        let (_host, services) = stand_in(Ok(release(&newer(), security)));
        let (mut services, downloads) = downloading(services, download);
        services.update_start = update_start;
        let mut app = start(services, &on(), NOW);
        for _ in 0..3 {
            frame(&mut app);
        }
        (app, downloads)
    }

    #[test]
    fn update_now_downloads_then_closes_and_installs() {
        let (mut app, downloads) = found_app(false, UpdateStart::default(), Ok(PathBuf::from("/updates/x")));
        assert_eq!(update_state(&app)["install"]["setting"], "on");
        assert!(downloads.lock().unwrap().is_empty());
        assert_eq!(app.run(RESTART, json!({})).unwrap()["pending"], "download");
        assert_eq!(*downloads.lock().unwrap(), [newer()]);
        assert!(!app.quit_requested, "not before the download is checked");
        frame(&mut app);
        assert!(app.quit_requested);
        assert_eq!(app.update_on_exit(), Some(OnExit::Install { version: version(&newer()), restart: true }));
    }

    #[test]
    fn a_download_that_fails_or_is_cancelled_keeps_mailcraft_open() {
        let (mut app, downloads) = found_app(false, UpdateStart::default(), Err("GitHub took too long".into()));
        app.run(RESTART, json!({})).unwrap();
        frame(&mut app);
        assert!(!app.quit_requested);
        assert_eq!(app.updates.download_failed.as_deref(), Some("GitHub took too long"));
        assert!(app.update_on_exit().is_none());
        // Try again asks once more.
        app.run(RESTART, json!({})).unwrap();
        assert_eq!(downloads.lock().unwrap().len(), 2);
        // Cancelled while it downloads: what comes is kept, and nothing closes.
        let (mut app, _) = found_app(false, UpdateStart::default(), Ok(PathBuf::from("/updates/x")));
        app.run(RESTART, json!({})).unwrap();
        app.updates.installing = false;
        frame(&mut app);
        assert!(!app.quit_requested);
        assert!(app.update_ready().is_some());
        assert!(app.update_on_exit().is_none(), "quitting installs nothing");
        // Pressed again: no second download, it closes and installs.
        app.run(RESTART, json!({})).unwrap();
        assert!(app.quit_requested);
        assert!(matches!(app.update_on_exit(), Some(OnExit::Install { restart: true, .. })));
    }

    #[test]
    fn update_now_needs_a_copy_that_updates_itself_and_a_release_found_this_run() {
        // The organisation turned it off.
        let (_host, mut services) = stand_in(Ok(release(&newer(), false)));
        services.updates_turned_off_by_organisation = true;
        let (services, downloads) = downloading(services, Ok(PathBuf::new()));
        let mut app = start(services, &on(), NOW);
        for _ in 0..3 {
            frame(&mut app);
        }
        assert_eq!(update_state(&app)["install"]["setting"], "organisation");
        assert!(app.run(RESTART, json!({})).is_err());
        assert!(downloads.lock().unwrap().is_empty());
        // A copy that doesn't update itself.
        let (_host, services) = stand_in(Ok(release(&newer(), false)));
        let mut app = start(services, &on(), NOW);
        frame(&mut app);
        assert_eq!(update_state(&app)["install"]["setting"], "notThisCopy");
        assert!(app.run(RESTART, json!({})).is_err());
        // Only told of in an earlier run: the window checks first.
        let (_host, services) = stand_in(Ok(release(&newer(), false)));
        let (services, downloads) = downloading(services, Ok(PathBuf::new()));
        let mut app = start(services, &UiState { update_told: newer(), update_checked: NOW, ..on() }, NOW);
        frame(&mut app);
        assert!(app.newer_release().is_some());
        assert!(app.run(RESTART, json!({})).is_err());
        assert!(downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn update_now_is_the_owners_click_alone() {
        let (mut app, downloads) = found_app(false, UpdateStart::default(), Ok(PathBuf::from("/updates/x")));
        // A script, through the control channel: refused, and nothing changes.
        assert_eq!(app.execute(RESTART, json!({})), Err(OWNERS_CLICK.to_string()));
        assert!(!app.quit_requested);
        // An agent, through the gate as the MCP bridge sends it: never.
        assert_eq!(mailcraft_engine::agent::access(RESTART), Some(mailcraft_engine::agent::Access::Never));
        assert_eq!(mailcraft_engine::agent::access(GO_BACK), Some(mailcraft_engine::agent::Access::Never));
        let (request, _answer) = crate::ControlRequest::new("engine.execute", json!({"command": RESTART, "params": {}}));
        let request = request.from_agent(Vec::new());
        let Some(crate::control::Outcome::Done(reply)) = crate::agent_gate::screen(&mut app, &request, &[]) else { panic!("settled by the gate") };
        assert_eq!(reply["ok"], false, "{reply}");
        frame(&mut app);
        assert!(!app.quit_requested);
        assert!(downloads.lock().unwrap().is_empty());
        assert!(app.update_on_exit().is_none());
    }

    #[test]
    fn a_version_that_didnt_start_offers_the_one_before() {
        let here = env!("CARGO_PKG_VERSION");
        let offer = UpdateStart { didnt_start: Some((here.to_string(), "0.1.0".to_string())), ..Default::default() };
        let (mut app, _) = found_app(false, offer.clone(), Ok(PathBuf::new()));
        assert_eq!(update_state(&app)["install"]["didntStart"]["previous"], "0.1.0");
        assert_eq!(app.execute(GO_BACK, json!({})), Err(OWNERS_CLICK.to_string()));
        assert_eq!(app.run(GO_BACK, json!({})).unwrap()["version"], "0.1.0");
        assert!(app.quit_requested);
        assert_eq!(app.update_on_exit(), Some(OnExit::GoBack { to: "0.1.0".into() }));
        assert_eq!(app.prefs().update_skip, here);
        // Put away: not offered again in this run, and nothing is gone back to.
        let (mut kept, _) = found_app(false, offer, Ok(PathBuf::new()));
        kept.run("ui.dismissUpdateNotice", json!({"notice": "didntStart"})).unwrap();
        assert!(kept.go_back_offer().is_none());
        assert!(kept.run(GO_BACK, json!({})).is_err());
        assert!(kept.update_on_exit().is_none());
        assert!(kept.run("ui.dismissUpdateNotice", json!({"notice": "installsItself"})).is_err());
        assert_eq!(mailcraft_engine::agent::access("ui.dismissUpdateNotice"), Some(mailcraft_engine::agent::Access::Safe));
        // A hostile record from the host is no offer.
        let (mut odd, _) =
            found_app(false, UpdateStart { didnt_start: Some(("x".into(), "0.1.0\r\n".into())), ..Default::default() }, Ok(PathBuf::new()));
        assert!(odd.go_back_offer().is_none());
        assert!(odd.run(GO_BACK, json!({})).is_err());
    }

    #[test]
    fn an_update_that_wasnt_installed_is_said_at_the_next_start() {
        let (mut app, _) = found_app(false, UpdateStart { not_installed: Some("9.0.0".into()), ..Default::default() }, Ok(PathBuf::new()));
        let text = "MailCraft 9.0.0 wasn't installed: this version is still the one here.";
        assert_eq!(app.updates.start_notice.as_deref(), Some(text));
        app.status_msg = None;
        frame(&mut app);
        assert!(said(&app).is_none(), "once");
        let (gone, _) = found_app(false, UpdateStart { not_gone_back: Some("0.1.0".into()), ..Default::default() }, Ok(PathBuf::new()));
        assert!(gone.updates.start_notice.as_deref().is_some_and(|s| s.starts_with("Going back to MailCraft 0.1.0 didn't happen")));
    }

    fn harness(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1400.0, 800.0))
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(4);
        harness
    }

    #[test]
    fn the_question_options_and_about_are_laid_out() {
        let (_host, services) = stand_in(Ok(release(&newer(), true)));
        let mut harness = harness(start(services, &UiState::default(), NOW));
        assert!(harness.state().updates.asking);
        assert!(harness.state().hit.rect("updateYes", 0).is_some());
        harness.state_mut().run("ui.updateCheck", json!({"value": true})).unwrap();
        harness.run_steps(4);
        assert!(harness.state().newer_release().is_some_and(|r| r.security));
        for page in ["options", "about"] {
            harness.state_mut().run("ui.backstage", json!({"value": true, "page": page})).unwrap();
            harness.run_steps(4);
        }
        assert!(harness.state().hit.rect("checkForUpdates", 0).is_some());
        assert!(harness.state().hit.rect("updateOption", 0).is_some());
    }

    #[test]
    fn the_update_button_opens_the_window_with_update_now() {
        let here = env!("CARGO_PKG_VERSION");
        for update_start in [UpdateStart::default(), UpdateStart { didnt_start: Some((here.into(), "0.1.0".into())), ..Default::default() }] {
            let offer = update_start.didnt_start.is_some();
            let (app, _) = found_app(false, update_start, Ok(PathBuf::from("/updates/x")));
            let mut harness = harness(app);
            let button = harness.state().hit.rect("updateButton", 0).expect("the update button").center();
            assert_eq!(harness.state().hit.rect("goBack", 0).is_some(), offer);
            assert!(harness.state().hit.rect("updateNow", 0).is_none(), "no window by itself");
            crate::people_view::testing::click(&mut harness, button, egui::PointerButton::Primary, 1);
            harness.run_steps(4);
            assert!(harness.state().updates.dialog);
            assert!(harness.state().hit.rect("updateNow", 0).is_some());
            assert!(harness.state().hit.rect("updateLater", 0).is_some());
            let later = harness.state().hit.rect("updateLater", 0).unwrap().center();
            crate::people_view::testing::click(&mut harness, later, egui::PointerButton::Primary, 1);
            harness.run_steps(3);
            assert!(!harness.state().updates.dialog);
            assert!(!harness.state().quit_requested);
        }
        // A copy that doesn't update itself gets the release's page instead.
        let (_host, services) = stand_in(Ok(release(&newer(), false)));
        let mut harness = harness(start(services, &on(), NOW));
        harness.state_mut().run("ui.dialog", json!({"name": DIALOG})).unwrap();
        harness.run_steps(4);
        assert!(harness.state().hit.rect("updatePage", 0).is_some());
        assert!(harness.state().hit.rect("updateNow", 0).is_none());
    }
}
