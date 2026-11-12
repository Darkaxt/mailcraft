// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attachments scanned by Microsoft Defender once the owner reads them, and flagged when it
//! finds something: nothing is deleted, hidden or moved.
//!
//! - **When:** a message the owner reads (the reading pane, a window of its own) has each of its
//!   files (inline ones too) of up to [`MOST_BYTES`] scanned that has no current verdict, one at a
//!   time ([`notice`], [`tick`]); `ui.scanAttachment` scans one again now. An agent reading a
//!   message (`mail.get`) scans nothing, and neither does a headless run.
//! - **How:** the window reads the bytes (decoded) and hands them to the host
//!   ([`Services::scan_file`](crate::Services::scan_file)), which scans a copy outside the
//!   mailbox's folders and only reports. The mailbox's files are never touched.
//! - **What is kept:** by the file's content (its SHA-256), in `<mailbox>.scans.json` beside the
//!   mailbox file, written whole and then put in place ([`sidecar`]): clean or flagged, the
//!   threats, when. A scan that failed is not kept. A clean file is scanned again when it is read
//!   [`CLEAN_FOR`] later; a flagged one stays flagged.
//! - **What it does:** a flagged file's chip warns, the message says so above its files, the
//!   owner is asked before opening or saving it, and an agent can't save it
//!   ([`agent_refusal`]). `mail.get` says what is known of each file (`scan`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::{Attachment, Id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::MailApp;

pub(crate) const SCAN: &str = "ui.scanAttachment";

/// Who scans.
pub const ENGINE: &str = "Microsoft Defender";

/// The largest file scanned.
pub const MOST_BYTES: u64 = 256 * 1024 * 1024;

/// How long a clean verdict holds, in seconds: a file read later is scanned again.
pub const CLEAN_FOR: i64 = 7 * 24 * 60 * 60;

/// Most files waiting to be scanned.
const MOST_WAITING: usize = 1_000;
/// Most verdicts read back from the file, and the largest such file read.
const MOST_KEPT: usize = 100_000;
const MOST_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Most content hashes of files in memory remembered between frames.
const MOST_HASHES: usize = 4_096;

/// A file to scan: its name (made safe by the host) and its bytes, decoded.
pub struct ScanRequest {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
}

/// What the host's scan found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanOutcome {
    Clean,
    /// Threats found, by name (or the copy taken away by real-time protection, said as such).
    Flagged {
        threats: Vec<String>,
    },
    /// No verdict: why.
    Failed {
        why: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Clean,
    Flagged,
}

/// A verdict kept for a file's content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub verdict: Verdict,
    #[serde(default)]
    pub threats: Vec<String>,
    /// When, in seconds since 1970.
    pub at: i64,
    #[serde(default)]
    pub engine: String,
}

/// The verdicts file as read.
#[derive(Deserialize)]
struct Kept {
    #[serde(default)]
    scans: BTreeMap<String, Known>,
}

struct Waiting {
    message: Id,
    index: usize,
    hash: String,
}

/// The verdicts of the open mailbox and the scans waiting and under way.
#[derive(Default)]
pub struct Scans {
    known: BTreeMap<String, Known>,
    /// The mailbox file `known` is for (`Some(None)`: one that isn't kept in a file).
    loaded: Option<Option<PathBuf>>,
    waiting: VecDeque<Waiting>,
    running: Option<(String, Receiver<ScanOutcome>)>,
    /// Files whose scan failed in this run, with why: not scanned again by themselves.
    failed: HashMap<String, String>,
    /// The content hashes of files in memory, by message, index, where the bytes are and how many.
    hashes: HashMap<(Id, usize, usize, usize), String>,
}

impl Scans {
    /// The hash of attachment `index` of message `id`, remembered for a file in memory.
    fn hash(&mut self, id: Id, index: usize, a: &Attachment) -> Option<String> {
        if a.kept.is_some() {
            return hash_of(a);
        }
        let key = memory_key(id, index, a);
        if let Some(hash) = self.hashes.get(&key) {
            return Some(hash.clone());
        }
        let hash = hash_of(a)?;
        if self.hashes.len() >= MOST_HASHES {
            self.hashes.clear();
        }
        self.hashes.insert(key, hash.clone());
        Some(hash)
    }

    /// As [`hash`](Self::hash), remembering nothing.
    fn hash_now(&self, id: Id, index: usize, a: &Attachment) -> Option<String> {
        if a.kept.is_none()
            && let Some(hash) = self.hashes.get(&memory_key(id, index, a))
        {
            return Some(hash.clone());
        }
        hash_of(a)
    }

    /// Whether `hash` has a verdict that holds at `now`.
    fn current(&self, hash: &str, now: i64) -> bool {
        self.known.get(hash).is_some_and(|k| k.verdict == Verdict::Flagged || now.saturating_sub(k.at) < CLEAN_FOR)
    }

    fn pending(&self, hash: &str) -> bool {
        self.running.as_ref().is_some_and(|(h, _)| h == hash) || self.waiting.iter().any(|w| w.hash == hash)
    }
}

fn memory_key(id: Id, index: usize, a: &Attachment) -> (Id, usize, usize, usize) {
    (id, index, Arc::as_ptr(&a.data) as usize, a.data.len())
}

/// The SHA-256 of a file's content, in hex: the one the mailbox keeps it under, else worked out.
pub fn hash_of(a: &Attachment) -> Option<String> {
    match &a.kept {
        Some(k) => Some(k.sha256.iter().map(|b| format!("{b:02x}")).collect()),
        None if a.data.is_empty() => None,
        None => mailcraft_engine::io::file_sha256(&a.data),
    }
}

/// Where the verdicts of the mailbox file `mailbox` are kept: `<mailbox>.scans.json`.
pub fn sidecar(mailbox: &Path) -> PathBuf {
    let mut name = mailbox.as_os_str().to_owned();
    name.push(".scans.json");
    PathBuf::from(name)
}

/// The verdicts kept for `mailbox`; none when there is no such file or it can't be read.
pub fn read(mailbox: &Path) -> BTreeMap<String, Known> {
    let path = sidecar(mailbox);
    let Ok(meta) = std::fs::metadata(&path) else { return BTreeMap::new() };
    if !meta.is_file() || meta.len() > MOST_FILE_BYTES {
        log::warn!("the attachment scans file isn't one that can be read");
        return BTreeMap::new();
    }
    let kept = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Kept>(&bytes).ok());
    let Some(kept) = kept else {
        log::warn!("the attachment scans file couldn't be read");
        return BTreeMap::new();
    };
    kept.scans.into_iter().filter(|(hash, _)| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).take(MOST_KEPT).collect()
}

/// Write `known` as the verdicts of `mailbox`: whole, into a file beside it, then put in place.
pub fn write(mailbox: &Path, known: &BTreeMap<String, Known>) -> std::io::Result<()> {
    let path = sidecar(mailbox);
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    #[derive(Serialize)]
    struct Written<'a> {
        version: u32,
        scans: &'a BTreeMap<String, Known>,
    }
    let bytes = serde_json::to_vec_pretty(&Written { version: 1, scans: known }).map_err(std::io::Error::other)?;
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// The verdicts are those of the mailbox open now: read when another opens.
fn ensure_loaded(app: &mut MailApp) {
    let path = app.session.path.clone();
    if app.scans.loaded.as_ref() == Some(&path) {
        return;
    }
    app.scans.known = path.as_deref().map(read).unwrap_or_default();
    app.scans.waiting.clear();
    app.scans.running = None;
    app.scans.failed.clear();
    app.scans.hashes.clear();
    app.scans.loaded = Some(path);
}

/// What is known of attachment `index` of message `id`, for the mailbox open now.
fn known_for(app: &MailApp, id: Id, index: usize) -> Option<&Known> {
    if app.scans.loaded.as_ref() != Some(&app.session.path) {
        return None;
    }
    let a = app.session.mailbox.message(id)?.attachments.get(index)?;
    let hash = app.scans.hash_now(id, index, a)?;
    app.scans.known.get(&hash)
}

/// The threats Microsoft Defender found in attachment `index` of message `id`, said in one line;
/// `None` when it wasn't flagged.
pub(crate) fn flagged(app: &MailApp, id: Id, index: usize) -> Option<String> {
    let k = known_for(app, id, index)?;
    (k.verdict == Verdict::Flagged).then(|| if k.threats.is_empty() { tl!("a threat").to_string() } else { k.threats.join(", ") })
}

/// Whether attachment `index` of message `id` is waiting to be scanned, or being scanned.
pub(crate) fn scanning(app: &MailApp, id: Id, index: usize) -> bool {
    let Some(a) = app.session.mailbox.message(id).and_then(|m| m.attachments.get(index)) else { return false };
    app.scans.hash_now(id, index, a).is_some_and(|hash| app.scans.pending(&hash))
}

/// The owner is reading message `id` (the reading pane or a window of its own shows it): each of
/// its files without a verdict that holds waits to be scanned.
pub(crate) fn notice(app: &mut MailApp, id: Id) {
    if app.services.scan_file.is_none() || app.is_headless() || id == 0 {
        return;
    }
    ensure_loaded(app);
    let now = app.session.now();
    let Some(m) = app.session.mailbox.message(id) else { return };
    let scans = &mut app.scans;
    for (index, a) in m.attachments.iter().enumerate() {
        let size = a.size() as u64;
        if size == 0 || size > MOST_BYTES {
            continue;
        }
        let Some(hash) = scans.hash(id, index, a) else { continue };
        if scans.current(&hash, now) || scans.failed.contains_key(&hash) || scans.pending(&hash) {
            continue;
        }
        if scans.waiting.len() >= MOST_WAITING {
            return;
        }
        scans.waiting.push_back(Waiting { message: id, index, hash });
    }
}

/// Once a frame: a scan that has ended is taken in, and the next one waiting starts.
pub(crate) fn tick(app: &mut MailApp) {
    if app.services.scan_file.is_none() {
        return;
    }
    ensure_loaded(app);
    if let Some((hash, rx)) = &app.scans.running {
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => ScanOutcome::Failed { why: "the scan stopped unexpectedly".to_string() },
        };
        let hash = hash.clone();
        app.scans.running = None;
        record(app, &hash, outcome);
    }
    while let Some(next) = app.scans.waiting.pop_front() {
        let Some(a) = app.session.mailbox.message(next.message).and_then(|m| m.attachments.get(next.index)) else { continue };
        // Still the file it was queued for (an attachment removed meanwhile moves the others up).
        if app.scans.hash(next.message, next.index, a).as_deref() != Some(next.hash.as_str()) {
            continue;
        }
        let name = a.name.clone();
        let bytes = match app.session.content(next.message, mailcraft_engine::Part::Attachment(next.index)) {
            Ok(bytes) => bytes,
            Err(e) => {
                app.scans.failed.insert(next.hash, e.to_string());
                continue;
            }
        };
        let Some(scan) = &app.services.scan_file else { return };
        let rx = scan(ScanRequest { name, bytes });
        app.scans.running = Some((next.hash, rx));
        return;
    }
}

/// Keep what a scan of the file `hash` found (a failure is only remembered for this run).
fn record(app: &mut MailApp, hash: &str, outcome: ScanOutcome) {
    let at = app.session.now();
    let (verdict, threats) = match outcome {
        ScanOutcome::Clean => (Verdict::Clean, Vec::new()),
        ScanOutcome::Flagged { threats } => (Verdict::Flagged, threats),
        ScanOutcome::Failed { why } => {
            log::info!("an attachment couldn't be scanned: {why}");
            app.scans.failed.insert(hash.to_string(), why);
            return;
        }
    };
    app.scans.failed.remove(hash);
    app.scans.known.insert(hash.to_string(), Known { verdict, threats, at, engine: ENGINE.to_string() });
    if verdict == Verdict::Flagged {
        app.status(tl!("Microsoft Defender flagged an attachment. The file is kept unchanged."));
    }
    if let Some(path) = app.session.path.clone()
        && let Err(e) = write(&path, &app.scans.known)
    {
        log::warn!("the attachment scans couldn't be kept ({:?})", e.kind());
    }
}

/// `ui.scanAttachment {id?, index}`: scan the file again now, whatever is known of it.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id != SCAN {
        return None;
    }
    Some(scan_now(app, p))
}

fn scan_now(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    if app.services.scan_file.is_none() || app.is_headless() {
        return Err(tl!("Attachments can't be scanned here: Microsoft Defender's scanner isn't available.").to_string());
    }
    ensure_loaded(app);
    let (message, index, _) = crate::attachment_open::target(app, p)?;
    let a = app
        .session
        .mailbox
        .message(message)
        .and_then(|m| m.attachments.get(index))
        .ok_or_else(|| tl!("The message has no such attachment").to_string())?;
    if a.size() as u64 > MOST_BYTES {
        return Err(tl!("The attachment is too large to be scanned").to_string());
    }
    let hash = app.scans.hash(message, index, a).ok_or_else(|| tl!("The attachment is empty").to_string())?;
    app.scans.failed.remove(&hash);
    if !app.scans.pending(&hash) {
        app.scans.waiting.push_front(Waiting { message, index, hash: hash.clone() });
    }
    Ok(json!({"queued": true, "hash": hash}))
}

/// The files of message `id` (its listed ones, or the one `index` names) that were flagged, by
/// their shown names, with what was found.
fn flagged_files(app: &MailApp, id: Id, index: Option<usize>) -> Vec<(String, String)> {
    let Some(m) = app.session.mailbox.message(id) else { return Vec::new() };
    let named: Vec<(usize, &Attachment)> = match index {
        Some(i) => m.attachments.get(i).map(|a| vec![(i, a)]).unwrap_or_default(),
        None => m.listed_attachments().collect(),
    };
    named.into_iter().filter_map(|(i, a)| flagged(app, id, i).map(|threats| (mailcraft_engine::opening::file_name(&a.name), threats))).collect()
}

/// The message and the attachment a save names (`index` only for Save As).
fn save_target(app: &MailApp, id: &str, params: &Value) -> Option<(Id, Option<usize>)> {
    match id {
        "mail.saveAttachment" => crate::attachment_open::target(app, params).ok().map(|(m, i, _)| (m, Some(i))),
        "mail.saveAllAttachments" => {
            let m = mailcraft_engine::p::message_strict(params).ok()?.or(app.session.sel.focus())?;
            Some((m, None))
        }
        _ => None,
    }
}

/// What the owner is asked before saving a file Microsoft Defender flagged (Save As, Save All
/// Attachments), with the params it then goes ahead with (naming the message); `None` for
/// anything else.
pub(crate) fn save_question(app: &MailApp, id: &str, params: &Value) -> Option<(Value, crate::dialogs::Question)> {
    let (message, index) = save_target(app, id, params)?;
    let files = flagged_files(app, message, index);
    if files.is_empty() {
        return None;
    }
    let names = files.iter().map(|(name, threats)| format!("{name} ({threats})")).collect::<Vec<_>>().join(", ");
    let text = crate::i18n::fmt(
        tl!("Microsoft Defender flagged {files}. The file is kept unchanged. Save it only if you are sure it is safe."),
        &[("files", &names)],
    );
    let mut params = if params.is_object() { params.clone() } else { json!({}) };
    if let Some(o) = params.as_object_mut() {
        o.insert("id".into(), json!(message));
    }
    Some((params, crate::dialogs::Question::new(tl!("Saving Mail Attachment"), text, tl!("Save anyway"), Value::Null)))
}

/// Why an agent may not run `id`: it saves a file Microsoft Defender flagged.
pub(crate) fn agent_refusal(app: &MailApp, id: &str, params: &Value) -> Option<String> {
    let (message, index) = save_target(app, id, params)?;
    let files = flagged_files(app, message, index);
    if files.is_empty() {
        return None;
    }
    let names = files.iter().map(|(name, threats)| format!("{name} ({threats})")).collect::<Vec<_>>().join(", ");
    Some(format!("Microsoft Defender flagged {names}: an agent can't save it. The owner can, in the window. Nothing was done"))
}

/// `mail.get`'s answer with what is known of each of its files (`scan`: verdict, threats, when).
pub(crate) fn annotate(app: &mut MailApp, id: &str, r: Result<Value, String>) -> Result<Value, String> {
    if id != "mail.get" {
        return r;
    }
    let mut v = r?;
    ensure_loaded(app);
    let Some(message) = v.get("id").and_then(Value::as_u64) else { return Ok(v) };
    if let Some(files) = v.get_mut("attachments").and_then(Value::as_array_mut) {
        for f in files.iter_mut() {
            let Some(index) = f.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()) else { continue };
            let Some(k) = known_for(app, message, index) else { continue };
            let scan = json!({"verdict": k.verdict, "threats": k.threats, "at": k.at, "engine": k.engine});
            if let Some(o) = f.as_object_mut() {
                o.insert("scan".into(), scan);
            }
        }
    }
    Ok(v)
}

#[cfg(test)]
#[path = "attachment_scan_tests.rs"]
mod attachment_scan_tests;
