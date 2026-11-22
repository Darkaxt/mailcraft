// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backups (v0.12.9): the mailbox copied, compressed, into a folder the owner picks, once a day.
//!
//! ```text
//! backup.json                     what is there: each file, its fingerprint, size and SHA-256
//! mail-2019.mailcraft.gz          the mail of a past year, as one self-contained mailbox file
//! mail-2026-11.mailcraft.gz       the mail of a month of this year
//! mail-undated.mailcraft.gz       mail without a date
//! mailbox-2026-11-16.mailcraft.gz the rest of the mailbox (accounts, folders, rules, contacts,
//!                                 calendars, tasks, settings) as it was that day; the last 7 kept
//! ```
//!
//! - **Light:** a file is written again only when the mail of its period changed (its
//!   fingerprint: what the mailbox file holds for each message, [`store::backup_fingerprint`]).
//!   A day without new mail in an old year leaves that year's file alone. At the turn of the
//!   year the months of the year before become one file for that year.
//! - **Safe:** each file is written beside its place, read back whole (the gzip's own check and
//!   a SHA-256 of what was written), and only then put in place of the one before. Files the
//!   manifest doesn't name are never touched.
//! - **Readable anywhere:** each `.mailcraft.gz` uncompresses to a mailbox file of the
//!   self-contained form (format 1) that MailCraft opens as it is; [`restore`] puts the whole
//!   mailbox back together from the folder.
//! - **No secrets:** passwords and sign-ins are kept by the system's credential store, never in
//!   the mailbox, so never in a backup.
//! - The search index is not backed up: it is made again from the mail.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use mailcraft_model::{ContentRef, Id, Mailbox, Message, Whole};
use mailcraft_time::Date;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cmd::archive;
use crate::store;

/// The manifest's name in the backup folder.
pub const MANIFEST: &str = "backup.json";

/// How many days' files of the rest of the mailbox are kept.
pub const STATES_KEPT: usize = 7;

const DAY: i64 = 86_400;

/// A backup made this recently (seconds) is not made again by the daily run.
pub const FRESH: i64 = 20 * 3600;

/// What the backup folder holds, as `backup.json` says.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: u32,
    /// When the last backup finished (seconds since 1970, UTC); 0: never.
    #[serde(default)]
    pub finished: i64,
    /// The mail, by period: `2019`, `2026-11`, `undated`.
    #[serde(default)]
    pub periods: BTreeMap<String, PeriodFile>,
    /// The files of the rest of the mailbox, oldest first.
    #[serde(default)]
    pub states: Vec<String>,
}

/// One period's file.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PeriodFile {
    pub file: String,
    pub fingerprint: String,
    pub messages: usize,
    /// The file's size and SHA-256, as written and read back.
    pub bytes: u64,
    pub sha256: String,
    /// When it was written (seconds since 1970, UTC).
    pub written: i64,
}

/// What a backup did.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Nothing done: the last backup is recent ([`FRESH`]).
    pub skipped: bool,
    /// When the backup (the last one, when skipped) finished.
    pub finished: i64,
    pub messages: usize,
    /// Period files in the folder, and those written this time.
    pub periods: usize,
    pub written: Vec<String>,
    /// Files taken away: months joined into their year, day files past [`STATES_KEPT`].
    pub removed: Vec<String>,
    /// Messages kept without some of their content, which couldn't be read.
    pub incomplete: usize,
    /// The size of the backup's files together.
    pub bytes: u64,
    /// The archived messages it holds whole (v0.12.10): [`crate::Session::note_backed_up`].
    #[serde(skip)]
    pub archived: Vec<Id>,
}

/// A backup to make: a copy of the mailbox (cheap: its messages are shared), where its file is
/// (for the content kept beside it), the folder to back up into and the time now.
pub struct Job {
    pub mailbox: Mailbox,
    pub path: PathBuf,
    pub folder: PathBuf,
    pub now: i64,
    /// Made even when the last backup is recent (Back up now).
    pub force: bool,
}

/// The period a message received at `at` belongs to, at `now`: its year for a past year, its
/// month for this year (and later), `undated` without a date. By UTC: the same on every machine.
pub fn period_of(at: i64, now: i64) -> String {
    if at <= 0 {
        return "undated".into();
    }
    let (d, n) = (Date::from_days(at.div_euclid(DAY)), Date::from_days(now.div_euclid(DAY)));
    if d.year < n.year { format!("{:04}", d.year) } else { format!("{:04}-{:02}", d.year, d.month) }
}

/// When a message came: received, else its own date.
fn when(m: &Message) -> i64 {
    if m.received > 0 { m.received } else { m.date }
}

/// Whether `name` is a file a backup writes: nothing else in the folder is ever removed, and a
/// name read from the manifest can't lead anywhere else.
fn ours(name: &str) -> bool {
    (name.starts_with("mail-") || name.starts_with("mailbox-"))
        && name.ends_with(".mailcraft.gz")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

/// The manifest in `folder`: none yet is an empty one.
pub fn read_manifest(folder: &Path) -> Result<Manifest, String> {
    let path = folder.join(MANIFEST);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn write_manifest(folder: &Path, manifest: &Manifest) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    store::write_atomic(&folder.join(MANIFEST), &bytes).map_err(|e| format!("{}: {e}", folder.join(MANIFEST).display()))
}

/// Make a backup ([`Job`]).
pub fn run(job: &Job) -> Result<Report, String> {
    let folder = &job.folder;
    // The folder the owner chose, as it is: one that isn't there (a drive not connected, a
    // folder moved) isn't made again somewhere the owner didn't choose (0.12.11).
    if !folder.is_dir() {
        return Err(format!("the backup folder isn't there: {}", folder.display()));
    }
    let mut manifest = read_manifest(folder)?;
    let total = |m: &Manifest| m.periods.values().map(|p| p.bytes).sum::<u64>();
    if !job.force && manifest.finished > 0 && (0..FRESH).contains(&(job.now - manifest.finished)) {
        return Ok(Report {
            skipped: true,
            finished: manifest.finished,
            messages: manifest.periods.values().map(|p| p.messages).sum(),
            periods: manifest.periods.len(),
            bytes: total(&manifest),
            ..Report::default()
        });
    }
    // The Local archive put away (`crate::parking`) is backed up as it is, taken out for the while.
    let whole;
    let job = if job.mailbox.parked.is_empty() {
        job
    } else {
        let mut mailbox = job.mailbox.clone();
        for (id, m) in crate::parking::thaw_all(&job.mailbox.parked)? {
            mailbox.messages.insert(id, m);
        }
        mailbox.parked = Default::default();
        whole = Job { mailbox, path: job.path.clone(), folder: job.folder.clone(), now: job.now, force: job.force };
        &whole
    };
    let mut groups: BTreeMap<String, Vec<Id>> = BTreeMap::new();
    for (id, m) in job.mailbox.messages.iter() {
        groups.entry(period_of(when(m), job.now)).or_default().push(*id);
    }
    let rest = rest_of(&job.mailbox, false)?;
    let mut report = Report::default();
    for (key, ids) in &groups {
        let fingerprint = fingerprint(&job.mailbox, ids)?;
        let file = format!("mail-{key}.mailcraft.gz");
        let there = |p: &PeriodFile| fs::metadata(folder.join(&p.file)).is_ok_and(|m| m.len() == p.bytes);
        // Archived mail whole here: in the file already, the same, or once it is written whole.
        let archived: Vec<Id> =
            ids.iter().copied().filter(|id| job.mailbox.messages.get(id).is_some_and(|m| archive::waiting(m) && archive::whole(m))).collect();
        if manifest.periods.get(key).is_some_and(|p| p.fingerprint == fingerprint && p.file == file && there(p)) {
            report.archived.extend(archived);
            continue;
        }
        let mut failed = HashSet::new();
        let (bytes, sha256, incomplete) = write_verified(&folder.join(&file), |out| write_period(out, &rest, job, ids, &mut failed))?;
        report.archived.extend(archived.into_iter().filter(|id| !failed.contains(id)));
        report.incomplete += incomplete;
        manifest.periods.insert(key.clone(), PeriodFile { file, fingerprint, messages: ids.len(), bytes, sha256, written: job.now });
        report.written.push(key.clone());
        // Each file as it is written: a backup cut short keeps what it did.
        write_manifest(folder, &manifest)?;
    }
    let day = Date::from_days(job.now.div_euclid(DAY)).iso();
    let state = format!("mailbox-{day}.mailcraft.gz");
    let whole_rest = rest_of(&job.mailbox, true)?;
    write_verified(&folder.join(&state), |out| write_json(out, &whole_rest).map(|()| 0))?;
    manifest.states.retain(|s| *s != state);
    manifest.states.push(state);
    // Months joined into their year, and periods with no mail left.
    let gone: Vec<String> = manifest.periods.keys().filter(|k| !groups.contains_key(*k)).cloned().collect();
    for key in gone {
        if let Some(p) = manifest.periods.remove(&key) {
            remove(folder, &p.file, &mut report.removed);
        }
    }
    while manifest.states.len() > STATES_KEPT {
        let old = manifest.states.remove(0);
        remove(folder, &old, &mut report.removed);
    }
    manifest.version = 1;
    manifest.finished = job.now;
    write_manifest(folder, &manifest)?;
    report.finished = job.now;
    report.messages = job.mailbox.messages.len();
    report.periods = manifest.periods.len();
    report.bytes = total(&manifest);
    Ok(report)
}

fn remove(folder: &Path, name: &str, removed: &mut Vec<String>) {
    if !ours(name) {
        log::warn!("backup: {name} isn't a backup file's name; left where it is");
        return;
    }
    match fs::remove_file(folder.join(name)) {
        Ok(()) => removed.push(name.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("backup: couldn't remove {name}: {e}"),
    }
}

/// The fingerprint of the messages `ids`.
fn fingerprint(mailbox: &Mailbox, ids: &[Id]) -> Result<String, String> {
    let mut sha = ring::digest::Context::new(&ring::digest::SHA256);
    for id in ids {
        let Some(m) = mailbox.messages.get(id) else { continue };
        sha.update(&store::backup_fingerprint(*id, m)?.bytes());
    }
    Ok(hex(sha.finish().as_ref()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The mailbox without its messages, as a JSON object; without its contacts, events and tasks
/// too unless `whole` (a period's file carries its mail and what it lies in: accounts, folders).
fn rest_of(mailbox: &Mailbox, whole: bool) -> Result<serde_json::Map<String, Value>, String> {
    let mut rest = mailbox.clone();
    rest.messages = Default::default();
    if !whole {
        (rest.contacts, rest.events, rest.tasks) = Default::default();
    }
    // The self-contained form.
    rest.version = 1;
    match serde_json::to_value(&rest).map_err(|e| e.to_string())? {
        Value::Object(map) => Ok(map),
        _ => Err("the mailbox isn't a JSON object".into()),
    }
}

fn write_json(out: &mut dyn Write, map: &serde_json::Map<String, Value>) -> Result<(), String> {
    serde_json::to_writer(out, map).map_err(|e| e.to_string())
}

/// A period's file: the mailbox's rest with the messages `ids`, each brought whole into memory
/// in turn and written out (a year of mail is never in memory at once). Returns how many were
/// written without some content that couldn't be read.
fn write_period(
    out: &mut dyn Write,
    rest: &serde_json::Map<String, Value>,
    job: &Job,
    ids: &[Id],
    failed: &mut HashSet<Id>,
) -> Result<usize, String> {
    let io = |e: std::io::Error| e.to_string();
    let json = |e: serde_json::Error| e.to_string();
    out.write_all(b"{").map_err(io)?;
    for (key, value) in rest.iter().filter(|(key, _)| *key != "messages") {
        serde_json::to_writer(&mut *out, key).map_err(json)?;
        out.write_all(b":").map_err(io)?;
        serde_json::to_writer(&mut *out, value).map_err(json)?;
        out.write_all(b",").map_err(io)?;
    }
    out.write_all(b"\"messages\":{").map_err(io)?;
    let mut incomplete = 0;
    let mut first = true;
    for id in ids {
        let Some(m) = job.mailbox.messages.get(id) else { continue };
        if !first {
            out.write_all(b",").map_err(io)?;
        }
        first = false;
        serde_json::to_writer(&mut *out, &id.to_string()).map_err(json)?;
        out.write_all(b":").map_err(io)?;
        let mut take = |r: &ContentRef| store::fetch_uncached(&job.path, r);
        match store::load_whole(*id, m, &mut take) {
            Ok((whole, _)) => serde_json::to_writer(&mut *out, &Whole(&whole)).map_err(json)?,
            Err(e) => {
                log::warn!("backup: {e}; kept without it");
                incomplete += 1;
                failed.insert(*id);
                serde_json::to_writer(&mut *out, &Whole(m)).map_err(json)?;
            }
        }
    }
    out.write_all(b"}}").map_err(io)?;
    Ok(incomplete)
}

/// A writer that keeps the SHA-256 and length of what goes through it.
struct Hashing<W> {
    inner: W,
    sha: ring::digest::Context,
    len: u64,
}

impl<W> Hashing<W> {
    fn new(inner: W) -> Hashing<W> {
        Hashing { inner, sha: ring::digest::Context::new(&ring::digest::SHA256), len: 0 }
    }

    fn done(self) -> (W, String, u64) {
        (self.inner, hex(self.sha.finish().as_ref()), self.len)
    }
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.sha.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.sha.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }
}

/// Write `path` through `write`, compressed, beside its place; read it back whole and check it;
/// then put it in place of the file before. Returns the file's size and SHA-256, and what
/// `write` returned.
fn write_verified(path: &Path, write: impl FnOnce(&mut dyn Write) -> Result<usize, String>) -> Result<(u64, String, usize), String> {
    let show = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let mut partial = path.as_os_str().to_os_string();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    let written = (|| {
        let file = store::create_private_replacing(&partial).map_err(|e| show(&e))?;
        let mut plain = Hashing::new(GzEncoder::new(BufWriter::new(file), Compression::default()));
        let answer = write(&mut plain)?;
        let (gz, plain_sha, plain_len) = plain.done();
        let file = gz.finish().map_err(|e| show(&e))?.into_inner().map_err(|e| show(&e.into_error()))?;
        file.sync_all().map_err(|e| show(&e))?;
        drop(file);
        // Read back: the gzip checks itself, and what comes out must be what went in.
        let mut packed = Hashing::new(BufReader::new(fs::File::open(&partial).map_err(|e| show(&e))?));
        let mut check = Hashing::new(std::io::sink());
        let mut gz = GzDecoder::new(&mut packed);
        std::io::copy(&mut gz, &mut check).map_err(|e| show(&format!("read back: {e}")))?;
        drop(gz);
        std::io::copy(&mut packed, &mut std::io::sink()).map_err(|e| show(&e))?;
        let (_, check_sha, check_len) = check.done();
        if (check_sha.as_str(), check_len) != (plain_sha.as_str(), plain_len) {
            return Err(show(&"read back, it isn't what was written"));
        }
        let (_, sha, len) = packed.done();
        Ok((len, sha, answer))
    })();
    match written {
        Ok(done) => {
            fs::rename(&partial, path).map_err(|e| show(&e))?;
            Ok(done)
        }
        Err(e) => {
            let _ = fs::remove_file(&partial);
            Err(e)
        }
    }
}

/// What [`restore`] put back.
#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Restored {
    pub messages: usize,
    pub periods: usize,
    /// The day file the rest of the mailbox came from.
    pub state: String,
}

/// Put the mailbox backed up in `folder` back together as a new mailbox file at `target` (with
/// its content folder beside it): the newest day file's accounts, folders, rules and the rest,
/// with the mail of every period. `target` must not be there yet: nothing is written over.
pub fn restore(folder: &Path, target: &Path) -> Result<Restored, String> {
    if target.exists() || store::blob_dir(target).exists() {
        return Err(format!("{} is already there; restore into a new file", target.display()));
    }
    let manifest = read_manifest(folder)?;
    let state = manifest.states.last().filter(|s| ours(s)).ok_or_else(|| format!("{}: no backup here", folder.display()))?;
    let mut mailbox = crate::io::read_mailbox(&gunzip(&folder.join(state))?)?;
    let mut restored = Restored { state: state.clone(), ..Restored::default() };
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for (key, p) in &manifest.periods {
        if !ours(&p.file) {
            return Err(format!("{}: the manifest names {}, which isn't a backup file", folder.display(), p.file));
        }
        let file = folder.join(&p.file);
        check(&file, p)?;
        // Each period through a mailbox file of its own beside the target, opened as any
        // self-contained file is (a large one as a stream, its content going beside it).
        let scratch = target.with_file_name(format!("{name}.restoring-{key}.mailcraft"));
        let result = (|| {
            let mut input = GzDecoder::new(BufReader::new(fs::File::open(&file).map_err(|e| format!("{}: {e}", file.display()))?));
            let mut output = store::create_private_replacing(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
            std::io::copy(&mut input, &mut output).map_err(|e| format!("{}: {e}", file.display()))?;
            drop(output);
            let opened = store::open(&scratch)?.mailbox;
            store::copy_content(&scratch, target, &opened)?;
            let ids: Vec<Id> = opened.messages.keys().copied().collect();
            for (id, m) in opened.messages.iter() {
                mailbox.messages.insert(*id, m.clone());
            }
            store::save(target, &mailbox)?;
            // What is safely beside the target now leaves memory.
            for id in &ids {
                let Some(m) = mailbox.messages.get(id) else { continue };
                if let Some(lean) = store::evictable(target, *id, m) {
                    mailbox.messages.insert(*id, Arc::new(lean));
                }
            }
            Ok::<usize, String>(ids.len())
        })();
        let _ = fs::remove_file(&scratch);
        let _ = fs::remove_dir_all(store::blob_dir(&scratch));
        restored.messages += result?;
        restored.periods += 1;
    }
    for line in mailbox.repair() {
        log::warn!("restored mailbox repaired: {line}");
    }
    store::save(target, &mailbox)?;
    Ok(restored)
}

/// That `file` is the one the manifest names: its size and SHA-256.
fn check(file: &Path, p: &PeriodFile) -> Result<(), String> {
    let mut read = Hashing::new(BufReader::new(fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?));
    std::io::copy(&mut read, &mut std::io::sink()).map_err(|e| format!("{}: {e}", file.display()))?;
    let (_, sha, len) = read.done();
    if (len, sha.as_str()) != (p.bytes, p.sha256.as_str()) {
        return Err(format!("{} is damaged: it isn't the file the backup wrote", file.display()));
    }
    Ok(())
}

/// The uncompressed bytes of a small backup file (a day file), within the mailbox file limit.
fn gunzip(file: &Path) -> Result<Vec<u8>, String> {
    let input = GzDecoder::new(BufReader::new(fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?));
    let mut bytes = Vec::new();
    input.take(crate::io::MAX_MAILBOX_JSON + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    if bytes.len() as u64 > crate::io::MAX_MAILBOX_JSON {
        return Err(format!("{}: too large", file.display()));
    }
    Ok(bytes)
}
