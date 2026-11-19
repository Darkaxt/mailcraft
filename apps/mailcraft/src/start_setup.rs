// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How MailCraft was last started on purpose, for a plain start (#fork, v0.12.20).
//!
//! The owner's own shortcut starts MailCraft with `--mailbox <file>`, `--control <port>` and
//! `--control-agents-only`, and names the file of the Google Desktop registration in
//! `MAILCRAFT_GOOGLE_CLIENT_FILE`. Everything else starts the program with none of that: the
//! installer's Start Menu entry and its Launch box, a `mailto:` link, a message or calendar file
//! opened from the file manager. Those used to open an empty mailbox of their own.
//!
//! So a start that names an absolute `--mailbox` keeps those settings in `start.json` in the
//! settings folder; a start that names no mailbox takes them back, as long as that mailbox file
//! is still there. It then meets the window that already has the mailbox and hands over to it
//! (`single_instance`), or opens it as the owner's shortcut would.
//!
//! It keeps the temporary folder too (`TEMP`), so a plain start writes its temporary files where
//! the owner's shortcut puts them rather than in the system's default one: MailCraft's own temporary
//! files go there (`tempfile::env::override_temp_dir`), and so do an update's installer's.
//!
//! `start.json` holds where things are, never a secret: the Google file's path, not what is in
//! it. Not kept or read without preferences (`MAILCRAFT_NO_PREFS`), for the sample, nor for a
//! windowless run.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The file in the settings folder.
pub const FILE: &str = "start.json";

/// The environment variable naming the Google Desktop registration's file: the JSON Google
/// gives for an installed app, `{"installed": {"client_id": …, "client_secret": …}}`.
pub const GOOGLE_FILE_ENV: &str = "MAILCRAFT_GOOGLE_CLIENT_FILE";

/// What a plain start takes back.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Setup {
    pub mailbox: PathBuf,
    pub control: Option<u16>,
    pub agents_only: bool,
    pub google_file: Option<PathBuf>,
    /// The temporary folder.
    pub temp: Option<PathBuf>,
}

/// Keep `setup` in `dir`, when its mailbox is named by its whole path. Best effort: a start never
/// fails over it. Rewritten only when something changed.
pub fn remember(dir: &Path, setup: &Setup) {
    let Some(mailbox) = setup.mailbox.to_str().filter(|_| setup.mailbox.is_absolute()) else { return };
    let (Some(google_file), Some(temp)) = (whole(&setup.google_file), whole(&setup.temp)) else { return };
    let value = json!({
        "schema": 1,
        "mailbox": mailbox,
        "control": setup.control,
        "agentsOnly": setup.agents_only,
        "googleClientFile": google_file,
        "temp": temp,
    });
    let path = dir.join(FILE);
    if std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).as_ref() == Some(&value) {
        return;
    }
    let bytes = serde_json::to_vec_pretty(&value).unwrap_or_default();
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|()| mailcraft_engine::store::write_atomic(&path, &bytes)) {
        log::debug!("how MailCraft was started was not kept: {e}");
    }
}

/// `Some(None)` for no path, `Some(Some(path))` for a whole UTF-8 path, `None` for another.
fn whole(path: &Option<PathBuf>) -> Option<Option<&str>> {
    match path {
        None => Some(None),
        Some(p) if p.is_absolute() => p.to_str().map(Some),
        Some(_) => None,
    }
}

/// What was kept in `dir`, when its mailbox file is still there.
pub fn recall(dir: &Path) -> Option<Setup> {
    let bytes = std::fs::read(dir.join(FILE)).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    if v.get("schema").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let mailbox = PathBuf::from(v.get("mailbox")?.as_str()?);
    if !mailbox.is_absolute() || !mailbox.is_file() {
        return None;
    }
    let control = match v.get("control") {
        None | Some(Value::Null) => None,
        Some(port) => Some(u16::try_from(port.as_u64()?).ok().filter(|p| *p != 0)?),
    };
    let path = |key: &str| v.get(key).and_then(Value::as_str).map(PathBuf::from).filter(|p| p.is_absolute());
    Some(Setup {
        mailbox,
        control,
        agents_only: v.get("agentsOnly").and_then(Value::as_bool) == Some(true),
        google_file: path("googleClientFile"),
        temp: path("temp"),
    })
}

/// The client id and secret in a Google Desktop registration file, checked as the environment's
/// are (`google_client_id`, `google_client_secret` in main.rs). Never logged.
pub fn google_client(file: &Path) -> Option<(String, String)> {
    let meta = std::fs::metadata(file).ok()?;
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    let v: Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let installed = v.get("installed")?;
    let id = installed.get("client_id")?.as_str()?.trim().to_string();
    let secret = installed.get("client_secret")?.as_str()?.to_string();
    let id_ok = !id.is_empty() && id.len() <= 200 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    let secret_ok = !secret.is_empty() && secret.len() <= 512 && secret.bytes().all(|b| b.is_ascii_graphic());
    (id_ok && secret_ok).then_some((id, secret))
}

#[cfg(test)]
#[path = "start_setup_tests.rs"]
mod start_setup_tests;
