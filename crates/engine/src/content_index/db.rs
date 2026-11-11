// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One app-owned writer, read-only WAL readers, and immutable located text.
//! This sidecar is derived data; no operation writes the primary mailbox.
//!
//! Schema 2 stores each text unit once (`raw`) behind an external-content
//! FTS5 index (`detail=none`, no column sizes) and folds at query time.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mailcraft_model::Id;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::matcher::{BestHit, Hit, Matcher, field_of};
use crate::SearchOptions;

const APPLICATION_ID: i64 = 0x4d435831; // MCX1, never inferred from a filename.
pub(crate) const SCHEMA: i64 = 2;
pub const SEGMENT_BYTES: usize = 256 * 1024;
pub const OVERLAP_BYTES: usize = 64 * 1024;
const BATCH_TIME: Duration = Duration::from_millis(250);

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metadata {
    pub id: Id,
    pub account: Id,
    pub folder: Id,
    pub content_fp: String,
    pub metadata_fp: String,
    pub row: Value,
    pub snippet: String,
    pub downloaded: bool,
    pub attachments: u64,
    pub complete_fields: u8,
    pub state: String,
    pub problem: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub location: Value,
    pub offset: u64,
    pub raw: String,
    pub starts_word: bool,
    pub ends_word: bool,
    pub derivation: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Link {
    pub document: Id,
    pub key: String,
    pub fields: u8,
    pub source: Value,
    pub available: bool,
}

pub struct Database {
    connection: Connection,
    /// When the open write batch began.
    batch: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct MatchFrame {
    pub content_fp: String,
    pub metadata_fp: String,
    pub complete_fields: u8,
    pub hits: u64,
    /// The best hit per positive query term (term index), for scoring.
    pub terms: Vec<(usize, BestHit)>,
}

fn error(why: impl std::fmt::Display) -> String {
    why.to_string()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
}

/// A network share or mapped network drive: its canonical path is UNC.
fn remote(path: &Path) -> bool {
    let probe = path.parent().unwrap_or(path);
    let Ok(canonical) = std::fs::canonicalize(probe) else { return false };
    let text = canonical.to_string_lossy();
    text.starts_with(r"\\?\UNC\") || (text.starts_with(r"\\") && !text.starts_with(r"\\?\"))
}

impl Database {
    /// The app-owned writer. An owned sidecar of an older schema is a
    /// rebuildable cache: it is deleted and built again. A file that is not an
    /// owned MailCraft index is preserved and refused.
    pub fn writer(path: &Path, stop: Arc<AtomicBool>) -> Result<Self> {
        if path.try_exists().map_err(error)? {
            let version = Self::owned_version(path)?;
            if version != SCHEMA {
                log::info!("content index: replacing the schema {version} sidecar with a fresh schema {SCHEMA} build");
                Self::retire(path)?;
            }
        }
        if !path.try_exists().map_err(error)? {
            // Initialize and close an unselected owned file before publishing
            // its name. A crash during schema creation cannot leave a selected
            // zero-identity database which future opens would have to guess at.
            let parent = path.parent().ok_or("the search sidecar has no parent directory")?;
            let temporary = tempfile::NamedTempFile::new_in(parent).map_err(error)?;
            let database = Self::open_file(temporary.path(), true)?;
            database.connection.close().map_err(|(_, why)| error(why))?;
            temporary.persist_noclobber(path).map_err(error)?;
        }
        let mut database = Self::open_file(path, false)?;
        // Long statements (prune, merge, optimize, a large part) observe the
        // owner's cancellation instead of holding the window's close.
        database.cancellation(stop)?;
        Ok(database)
    }

    fn regular(path: &Path) -> Result<()> {
        let metadata = std::fs::symlink_metadata(path).map_err(error)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("the search sidecar is not a regular file".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("the search sidecar must not be a reparse point".into());
            }
        }
        Ok(())
    }

    /// The schema version of an owned sidecar; refuses any other file.
    fn owned_version(path: &Path) -> Result<i64> {
        Self::regular(path)?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(error)?;
        let application: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0)).map_err(error)?;
        if application != APPLICATION_ID {
            return Err("the existing search-path file is not an owned MailCraft index; it was preserved".into());
        }
        let version = connection.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(error)?;
        connection.close().map_err(|(_, why)| error(why))?;
        Ok(version)
    }

    /// Delete an owned older-schema sidecar and its journal files.
    fn retire(path: &Path) -> Result<()> {
        for suffix in ["-wal", "-shm", "-journal"] {
            let companion = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
            match std::fs::remove_file(&companion) {
                Err(why) if why.kind() != std::io::ErrorKind::NotFound => return Err(error(why)),
                _ => {}
            }
        }
        std::fs::remove_file(path).map_err(error)
    }

    fn open_file(path: &Path, fresh: bool) -> Result<Self> {
        Self::regular(path)?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(error)?;
        let application: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0)).map_err(error)?;
        if !fresh && application != APPLICATION_ID {
            return Err("the existing search-path file is not an owned MailCraft index; it was preserved".into());
        }
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(error)?;
        if !fresh && version != SCHEMA {
            return Err("the search index has another schema version; rebuild it through its owner".into());
        }
        // WAL needs shared memory, which a network or mapped drive cannot
        // provide reliably ("disk I/O error"): a rollback journal there.
        let remote = remote(path);
        let journal = if remote { None } else { connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0)).ok() };
        if !journal.is_some_and(|mode| mode.eq_ignore_ascii_case("wal")) {
            let mode: String = connection.pragma_update_and_check(None, "journal_mode", "DELETE", |row| row.get(0)).map_err(error)?;
            log::info!("content index: journal_mode={mode} (network or mapped drive: {remote})");
        }
        connection
            .execute_batch(
                "PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;
            PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA secure_delete=FAST;
            PRAGMA cache_size=-65536; PRAGMA mmap_size=268435456; PRAGMA temp_store=MEMORY;
            CREATE TABLE IF NOT EXISTS metadata(
                id TEXT PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL,
                content_fp TEXT NOT NULL, metadata_fp TEXT NOT NULL,
                row_json TEXT NOT NULL, snippet TEXT NOT NULL,
                downloaded INTEGER NOT NULL, complete_fields INTEGER NOT NULL,
                state TEXT NOT NULL, problem TEXT, attachment_count INTEGER NOT NULL,
                failures INTEGER NOT NULL DEFAULT 0, retry_after INTEGER NOT NULL DEFAULT 0);
            CREATE INDEX IF NOT EXISTS metadata_folder ON metadata(folder);
            CREATE INDEX IF NOT EXISTS metadata_account ON metadata(account);
            CREATE INDEX IF NOT EXISTS metadata_state ON metadata(state);
            CREATE TABLE IF NOT EXISTS parts(
                key TEXT PRIMARY KEY, state TEXT NOT NULL, complete INTEGER NOT NULL,
                coverage_json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS links(
                document TEXT NOT NULL REFERENCES metadata(id) ON DELETE CASCADE,
                part_key TEXT NOT NULL REFERENCES parts(key), fields INTEGER NOT NULL,
                source_json TEXT NOT NULL, available INTEGER NOT NULL,
                PRIMARY KEY(document,part_key,fields,source_json));
            CREATE INDEX IF NOT EXISTS links_part ON links(part_key);
            CREATE TABLE IF NOT EXISTS units(
                id INTEGER PRIMARY KEY, part_key TEXT NOT NULL REFERENCES parts(key),
                ordinal INTEGER NOT NULL, location_json TEXT NOT NULL,
                source_offset INTEGER NOT NULL, starts_word INTEGER NOT NULL,
                ends_word INTEGER NOT NULL, derivation TEXT NOT NULL,
                raw TEXT NOT NULL, UNIQUE(part_key,ordinal));
            CREATE VIRTUAL TABLE IF NOT EXISTS content_fts USING fts5(
                raw, content='units', content_rowid='id',
                tokenize='unicode61 remove_diacritics 2', detail=none, columnsize=0);
            CREATE VIRTUAL TABLE IF NOT EXISTS content_vocab USING fts5vocab(content_fts, 'row');
            CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .map_err(error)?;
        if fresh {
            connection
                .execute_batch(
                    "INSERT INTO content_fts(content_fts, rank) VALUES('automerge', 8);
                INSERT INTO content_fts(content_fts, rank) VALUES('crisismerge', 32);",
                )
                .map_err(error)?;
            connection.pragma_update(None, "application_id", APPLICATION_ID).map_err(error)?;
            connection.pragma_update(None, "user_version", SCHEMA).map_err(error)?;
        }
        Ok(Self { connection, batch: None })
    }

    pub fn reader(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(error)?;
        let application: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0)).map_err(error)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(error)?;
        if application != APPLICATION_ID || version != SCHEMA {
            return Err("the native search index is not initialized with the expected schema".into());
        }
        connection
            .execute_batch(
                "PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA busy_timeout=5000;
            PRAGMA cache_size=-16384; PRAGMA mmap_size=268435456; PRAGMA temp_store=MEMORY;",
            )
            .map_err(error)?;
        Ok(Self { connection, batch: None })
    }

    /// Start a write batch: many parts and documents share one transaction,
    /// committed by [`Database::commit_batch`] (about every 250 ms).
    pub fn begin_batch(&mut self) -> Result<()> {
        if self.batch.is_none() {
            self.connection.execute_batch("BEGIN IMMEDIATE").map_err(error)?;
            self.batch = Some(Instant::now());
        }
        Ok(())
    }

    /// Whether the open batch has run long enough to commit.
    pub fn batch_due(&self) -> bool {
        self.batch.is_some_and(|started| started.elapsed() >= BATCH_TIME)
    }

    /// Commit the open batch; answers how long the commit took.
    pub fn commit_batch(&mut self) -> Result<Duration> {
        let Some(_) = self.batch.take() else { return Ok(Duration::ZERO) };
        let started = Instant::now();
        if let Err(why) = self.connection.execute_batch("COMMIT") {
            let _ = self.connection.execute_batch("ROLLBACK");
            return Err(error(why));
        }
        Ok(started.elapsed())
    }

    /// Abandon the open batch (cancellation or a failure): nothing of it is
    /// kept, and its documents are indexed again later.
    pub fn rollback_batch(&mut self) {
        if self.batch.take().is_some() {
            // An interrupted statement may have rolled the transaction back
            // already; a second rollback then has nothing to do.
            let _ = self.connection.execute_batch("ROLLBACK");
        }
    }

    pub fn metadata(&mut self, rows: &[Metadata]) -> Result<()> {
        let transaction = self.connection.savepoint().map_err(error)?;
        for metadata in rows {
            // A new content version cannot keep links to an older body. A mere
            // read mark/move updates metadata without re-extracting attachments.
            transaction
                .execute(
                    "DELETE FROM links WHERE document=?1 AND EXISTS(
                SELECT 1 FROM metadata WHERE id=?1 AND content_fp<>?2)",
                    params![metadata.id.to_string(), metadata.content_fp],
                )
                .map_err(error)?;
            transaction
                .execute(
                    "INSERT INTO metadata(id,account,folder,content_fp,metadata_fp,row_json,snippet,
                downloaded,complete_fields,state,problem,attachment_count) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                ON CONFLICT(id) DO UPDATE SET account=excluded.account,folder=excluded.folder,
                content_fp=excluded.content_fp,metadata_fp=excluded.metadata_fp,row_json=excluded.row_json,
                snippet=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.snippet ELSE excluded.snippet END,
                downloaded=excluded.downloaded,attachment_count=excluded.attachment_count,
                complete_fields=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.complete_fields ELSE excluded.complete_fields END,
                state=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.state ELSE excluded.state END,
                problem=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.problem ELSE excluded.problem END,
                failures=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.failures ELSE 0 END,
                retry_after=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.retry_after ELSE 0 END",
                    params![
                        metadata.id.to_string(),
                        metadata.account.to_string(),
                        metadata.folder.to_string(),
                        metadata.content_fp,
                        metadata.metadata_fp,
                        metadata.row.to_string(),
                        metadata.snippet,
                        metadata.downloaded,
                        i64::from(metadata.complete_fields),
                        metadata.state,
                        metadata.problem,
                        i64::try_from(metadata.attachments).map_err(error)?
                    ],
                )
                .map_err(error)?;
        }
        transaction.commit().map_err(error)
    }

    pub fn document_ids(&self) -> Result<HashSet<Id>> {
        let mut statement = self.connection.prepare("SELECT id FROM metadata").map_err(error)?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(error)?
            .map(|value| value.map_err(error)?.parse::<Id>().map_err(error))
            .collect()
    }

    pub fn remove(&mut self, ids: impl IntoIterator<Item = Id>) -> Result<()> {
        let transaction = self.connection.savepoint().map_err(error)?;
        for id in ids {
            transaction.execute("DELETE FROM metadata WHERE id=?1", [id.to_string()]).map_err(error)?;
        }
        transaction.commit().map_err(error)
    }

    /// Queue failed and problem documents again. `all` (an app start): every
    /// failed/missing part and failed document; otherwise only failed
    /// documents whose bounded backoff ([`Database::record_failure`]) is due.
    pub fn retry_failed(&mut self, all: bool) -> Result<Vec<Id>> {
        let now = unix_now();
        let transaction = self.connection.savepoint().map_err(error)?;
        let mut ids = {
            let mut statement = transaction.prepare("SELECT id FROM metadata WHERE state='failed' AND (?1 OR retry_after<=?2)").map_err(error)?;
            statement
                .query_map(params![all, now], |row| row.get::<_, String>(0))
                .map_err(error)?
                .map(|row| row.map_err(error)?.parse::<Id>().map_err(error))
                .collect::<Result<Vec<_>>>()?
        };
        if all {
            let mut statement = transaction
                .prepare(
                    "SELECT DISTINCT document FROM links JOIN parts ON parts.key=links.part_key
                WHERE parts.state IN ('failed','missing')",
                )
                .map_err(error)?;
            for id in statement.query_map([], |row| row.get::<_, String>(0)).map_err(error)? {
                let id = id.map_err(error)?.parse::<Id>().map_err(error)?;
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        for id in &ids {
            transaction.execute("UPDATE metadata SET state='pending',complete_fields=0,problem=NULL WHERE id=?1", [id.to_string()]).map_err(error)?;
        }
        if all {
            transaction
                .execute("DELETE FROM links WHERE part_key IN (SELECT key FROM parts WHERE state IN ('failed','missing'))", [])
                .map_err(error)?;
        }
        transaction.commit().map_err(error)?;
        if all {
            self.prune()?;
        }
        Ok(ids)
    }

    /// A document failed outside one part's own gap: try it again after a
    /// bounded backoff (two minutes doubling, at most a day), and at the next
    /// app start.
    pub fn record_failure(&mut self, id: Id) -> Result<()> {
        let failures: i64 = self
            .connection
            .query_row("SELECT failures FROM metadata WHERE id=?1", [id.to_string()], |row| row.get(0))
            .optional()
            .map_err(error)?
            .unwrap_or(0);
        let backoff = 120i64.saturating_mul(1i64 << failures.clamp(0, 10)).min(24 * 60 * 60);
        self.connection
            .execute(
                "UPDATE metadata SET failures=failures+1,retry_after=?2 WHERE id=?1",
                params![id.to_string(), unix_now().saturating_add(backoff)],
            )
            .map_err(error)?;
        Ok(())
    }

    pub fn frames(&self) -> Result<HashMap<Id, MatchFrame>> {
        let mut statement = self.connection.prepare("SELECT id,content_fp,metadata_fp,complete_fields FROM metadata").map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let mut frames = HashMap::new();
        while let Some(row) = rows.next().map_err(error)? {
            let id: Id = row.get::<_, String>(0).map_err(error)?.parse().map_err(error)?;
            frames.insert(
                id,
                MatchFrame {
                    content_fp: row.get(1).map_err(error)?,
                    metadata_fp: row.get(2).map_err(error)?,
                    complete_fields: row.get(3).map_err(error)?,
                    hits: 0,
                    terms: Vec::new(),
                },
            );
        }
        Ok(frames)
    }

    pub fn cancellation(&mut self, cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<()> {
        self.connection.progress_handler(1000, Some(move || cancelled.load(std::sync::atomic::Ordering::Acquire))).map_err(error)
    }

    /// Retire derived text only after its last current document link goes away.
    /// Other accounts' identical attachment payloads keep their own live links.
    pub fn prune(&mut self) -> Result<()> {
        loop {
            let key: Option<String> = self
                .connection
                .query_row(
                    "SELECT key FROM parts
                WHERE NOT EXISTS(SELECT 1 FROM links WHERE links.part_key=parts.key) LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(error)?;
            let Some(key) = key else { return Ok(()) };
            let transaction = self.connection.savepoint().map_err(error)?;
            // External content: the index entry is removed with the text it
            // was built from, before that text goes.
            transaction
                .execute(
                    "INSERT INTO content_fts(content_fts,rowid,raw)
                SELECT 'delete',id,raw FROM units WHERE part_key=?1",
                    [&key],
                )
                .map_err(error)?;
            transaction.execute("DELETE FROM units WHERE part_key=?1", [&key]).map_err(error)?;
            transaction.execute("DELETE FROM parts WHERE key=?1", [&key]).map_err(error)?;
            transaction.commit().map_err(error)?;
        }
    }

    /// Idle FTS5 maintenance: `optimize` once after the first complete build,
    /// later a few bounded `merge` steps. Answers whether more merging is left.
    pub fn maintain(&mut self, optimize: bool, cancelled: &AtomicBool) -> Result<bool> {
        let optimized = self
            .connection
            .query_row("SELECT value FROM settings WHERE key='optimized'", [], |row| row.get::<_, String>(0))
            .optional()
            .map_err(error)?
            .is_some();
        if optimize && !optimized {
            let started = Instant::now();
            self.connection.execute("INSERT INTO content_fts(content_fts) VALUES('optimize')", []).map_err(error)?;
            self.connection.execute("INSERT OR REPLACE INTO settings VALUES('optimized','1')", []).map_err(error)?;
            log::info!("content index: optimized the full-text index in {} ms", started.elapsed().as_millis());
            return Ok(false);
        }
        for _ in 0..4 {
            if cancelled.load(Ordering::Acquire) {
                return Err("index maintenance cancelled".into());
            }
            let before: i64 = self.connection.query_row("SELECT total_changes()", [], |row| row.get(0)).map_err(error)?;
            self.connection.execute("INSERT INTO content_fts(content_fts, rank) VALUES('merge', 256)", []).map_err(error)?;
            let after: i64 = self.connection.query_row("SELECT total_changes()", [], |row| row.get(0)).map_err(error)?;
            // FTS5 reports a merge that found nothing to do as fewer than two changes.
            if after.saturating_sub(before) < 2 {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Bytes of the sidecar and its write-ahead log, for build instrumentation.
    pub fn size(path: &Path) -> u64 {
        let wal = std::path::PathBuf::from(format!("{}-wal", path.display()));
        [path.to_path_buf(), wal].iter().filter_map(|file| std::fs::metadata(file).ok()).map(|metadata| metadata.len()).sum()
    }

    pub fn part_ready(&self, key: &str) -> Result<Option<(String, bool, Value)>> {
        self.connection
            .query_row("SELECT state,complete,coverage_json FROM parts WHERE key=?1", [key], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?, row.get::<_, String>(2)?))
            })
            .optional()
            .map_err(error)?
            .map(|(state, complete, coverage)| Ok((state, complete, serde_json::from_str(&coverage).map_err(error)?)))
            .transpose()
    }

    /// The caller supplies a completed, verified extraction spool. No native
    /// parser/OCR process runs while this short commit holds the writer. The
    /// text is stored once (`raw`); folding happens at query time.
    pub fn part(
        &mut self,
        key: &str,
        state: &str,
        complete: bool,
        coverage: &Value,
        segments: impl IntoIterator<Item = Result<Segment>>,
    ) -> Result<()> {
        let transaction = self.connection.savepoint().map_err(error)?;
        // Existing completed content is immutable and source/version-addressed.
        if transaction.query_row("SELECT EXISTS(SELECT 1 FROM parts WHERE key=?1)", [key], |row| row.get::<_, bool>(0)).map_err(error)? {
            return Err("an existing immutable extraction key cannot be overwritten".into());
        }
        transaction.execute("INSERT INTO parts VALUES(?1,?2,?3,?4)", params![key, state, complete, coverage.to_string()]).map_err(error)?;
        {
            let mut unit = transaction
                .prepare(
                    "INSERT INTO units(part_key,ordinal,location_json,source_offset,
                starts_word,ends_word,derivation,raw) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                )
                .map_err(error)?;
            let mut indexed = transaction.prepare("INSERT INTO content_fts(rowid,raw) VALUES(?1,?2)").map_err(error)?;
            for (ordinal, segment) in segments.into_iter().enumerate() {
                let segment = segment?;
                if segment.raw.len() > SEGMENT_BYTES + OVERLAP_BYTES {
                    return Err("the extractor returned an oversized text segment".into());
                }
                let offset = i64::try_from(segment.offset).map_err(error)?;
                let ordinal = i64::try_from(ordinal).map_err(error)?;
                let id = unit
                    .insert(params![
                        key,
                        ordinal,
                        segment.location.to_string(),
                        offset,
                        segment.starts_word,
                        segment.ends_word,
                        segment.derivation,
                        segment.raw
                    ])
                    .map_err(error)?;
                indexed.execute(params![id, segment.raw]).map_err(error)?;
            }
        }
        transaction.commit().map_err(error)
    }

    pub fn link(&mut self, links: &[Link]) -> Result<()> {
        let transaction = self.connection.savepoint().map_err(error)?;
        for link in links {
            transaction
                .execute(
                    "INSERT OR REPLACE INTO links VALUES(?1,?2,?3,?4,?5)",
                    params![link.document.to_string(), link.key, i64::from(link.fields), link.source.to_string(), link.available],
                )
                .map_err(error)?;
        }
        transaction.commit().map_err(error)
    }

    pub fn current(&self, id: Id) -> Result<Option<(String, String, u8, String)>> {
        self.connection
            .query_row("SELECT content_fp,metadata_fp,complete_fields,state FROM metadata WHERE id=?1", [id.to_string()], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()
            .map_err(error)
    }

    /// A producer completion belongs only to the content version it actually
    /// read. A metadata update or replacement during extraction cannot be
    /// acknowledged as the newly indexed document.
    pub fn finish(&mut self, id: Id, content_fp: &str, complete_fields: u8, snippet: &str, state: &str, problem: Option<&str>) -> Result<bool> {
        self.connection
            .execute(
                "UPDATE metadata SET complete_fields=?3,snippet=?4,state=?5,problem=?6
            WHERE id=?1 AND content_fp=?2",
                params![id.to_string(), content_fp, i64::from(complete_fields), snippet, state, problem],
            )
            .map(|count| count == 1)
            .map_err(error)
    }

    pub fn begin_read(&self) -> Result<()> {
        self.connection.execute_batch("BEGIN DEFERRED").map_err(error)
    }

    pub fn end_read(&self) -> Result<()> {
        self.connection.execute_batch("ROLLBACK").map_err(error)
    }

    /// List rows come from persisted metadata, including computed previews;
    /// this path never opens a body or invokes an attachment renderer.
    pub fn row(&self, id: Id) -> Result<Option<Value>> {
        self.connection
            .query_row("SELECT row_json,snippet FROM metadata WHERE id=?1", [id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .optional()
            .map_err(error)?
            .map(|(row, snippet)| {
                let mut row: Value = serde_json::from_str(&row).map_err(error)?;
                let object = row.as_object_mut().ok_or("the indexed row is not metadata")?;
                object.insert("snippet".into(), Value::String(snippet));
                Ok(row)
            })
            .transpose()
    }

    /// Every vocabulary token with the number of units holding it (FTS5
    /// `fts5vocab` 'row'), for stage-1 substring expansion. Cached by the
    /// reader per index generation.
    pub fn vocabulary(&self, cancelled: &AtomicBool) -> Result<Vec<(Box<str>, u32)>> {
        let mut statement = self.connection.prepare("SELECT term,doc FROM content_vocab").map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let mut terms = Vec::new();
        while let Some(row) = rows.next().map_err(error)? {
            if terms.len() % 4096 == 0 && cancelled.load(Ordering::Acquire) {
                return Err("search cancelled".into());
            }
            let term: String = row.get(0).map_err(error)?;
            let documents: i64 = row.get(1).map_err(error)?;
            terms.push((term.into_boxed_str(), u32::try_from(documents).unwrap_or(u32::MAX)));
        }
        Ok(terms)
    }

    /// Units a bounded scan reads, for its progress.
    pub fn unit_count(&self) -> Result<u64> {
        let count: i64 = self.connection.query_row("SELECT COUNT(*) FROM units", [], |row| row.get(0)).map_err(error)?;
        Ok(u64::try_from(count).unwrap_or(0))
    }

    /// Stage 2: verify each candidate unit's stored text with `matcher` and
    /// keep each document's best hit in the parts `fields` names. Candidates
    /// come from the FTS5 `expression` (stage 1), or without one from a
    /// bounded, cancellable scan of every unit that reports its progress.
    pub fn matching(
        &self,
        matcher: &Matcher,
        expression: Option<&str>,
        fields: u8,
        cancelled: &AtomicBool,
        progress: &AtomicU64,
    ) -> Result<HashMap<Id, BestHit>> {
        let sql = if expression.is_some() {
            "SELECT u.id,u.raw,u.starts_word,u.ends_word,l.document,l.fields FROM content_fts
             JOIN units u ON u.id=content_fts.rowid JOIN links l ON l.part_key=u.part_key
             WHERE content_fts MATCH ?1 AND (l.fields & ?2)<>0 AND l.available=1"
        } else {
            "SELECT u.id,u.raw,u.starts_word,u.ends_word,l.document,l.fields FROM units u
             JOIN links l ON l.part_key=u.part_key WHERE ?1 IS NULL AND (l.fields & ?2)<>0 AND l.available=1"
        };
        let mut statement = self.connection.prepare(sql).map_err(error)?;
        let mut rows = statement.query(params![expression, i64::from(fields)]).map_err(error)?;
        let mut best: HashMap<Id, BestHit> = HashMap::new();
        // One unit linked from several documents (a shared attachment) is
        // verified once.
        let mut verified: HashMap<i64, Option<Hit>> = HashMap::new();
        while let Some(row) = rows.next().map_err(error)? {
            if cancelled.load(Ordering::Acquire) {
                return Err("search cancelled".into());
            }
            let unit: i64 = row.get(0).map_err(error)?;
            let hit = match verified.get(&unit) {
                Some(hit) => hit.clone(),
                None => {
                    progress.fetch_add(1, Ordering::Relaxed);
                    let raw: String = row.get(1).map_err(error)?;
                    let hit = matcher.best(&raw, row.get(2).map_err(error)?, row.get(3).map_err(error)?);
                    verified.insert(unit, hit.clone());
                    hit
                }
            };
            let Some(hit) = hit else { continue };
            let id: Id = row.get::<_, String>(4).map_err(error)?.parse().map_err(error)?;
            let link_fields: u8 = row.get(5).map_err(error)?;
            let candidate = BestHit { score: hit.score, matched: hit.matched, extra: hit.extra, field: field_of(link_fields & fields) };
            match best.get(&id) {
                Some(known) if !candidate.better_than(known) => {}
                _ => {
                    best.insert(id, candidate);
                }
            }
        }
        Ok(best)
    }

    pub fn sources(
        &self,
        document: Id,
        query: &mailcraft_model::search::Query,
        options: &SearchOptions,
        offset: usize,
        limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Value> {
        let (content_fp, metadata_fp, complete_fields, _) = self.current(document)?.ok_or("this message has not entered the persistent index")?;
        let mut total = 0usize;
        let mut items = Vec::new();
        let mut page_bytes = 0usize;
        let mut page_full = false;
        for index in 0..query.len() {
            let Some(term) = query.text_of(index) else { continue };
            if query.term_negated(index) {
                continue;
            }
            let fields = term.fields & options.fields();
            if fields == 0 {
                continue;
            }
            let matcher = Matcher::new(&term.source, term.case_sensitive, options)?;
            let mut statement = self
                .connection
                .prepare(
                    "SELECT part_key,source_json FROM links
                WHERE document=?1 AND (fields & ?2)<>0 AND available=1 ORDER BY part_key,source_json",
                )
                .map_err(error)?;
            let mut rows = statement.query(params![document.to_string(), i64::from(fields)]).map_err(error)?;
            while let Some(row) = rows.next().map_err(error)? {
                if cancelled.load(Ordering::Acquire) {
                    return Err("search source inspection cancelled".into());
                }
                let key: String = row.get(0).map_err(error)?;
                let source: String = row.get(1).map_err(error)?;
                let mut statement = self
                    .connection
                    .prepare(
                        "SELECT location_json,source_offset,derivation,raw,starts_word,ends_word
                    FROM units WHERE part_key=?1 ORDER BY ordinal",
                    )
                    .map_err(error)?;
                let mut units = statement.query([&key]).map_err(error)?;
                let mut found = None;
                while let Some(unit) = units.next().map_err(error)? {
                    if cancelled.load(Ordering::Acquire) {
                        return Err("search source inspection cancelled".into());
                    }
                    let raw: String = unit.get(3).map_err(error)?;
                    if let Some(hit) = matcher.best(&raw, unit.get(4).map_err(error)?, unit.get(5).map_err(error)?) {
                        found = Some((
                            unit.get::<_, String>(0).map_err(error)?,
                            u64::try_from(unit.get::<_, i64>(1).map_err(error)?).map_err(error)?,
                            unit.get::<_, String>(2).map_err(error)?,
                            hit,
                        ));
                        break;
                    }
                }
                if let Some((location, at, derivation, hit)) = found {
                    if total >= offset && items.len() < limit && !page_full {
                        let item = json!({"clause":index,"source":serde_json::from_str::<Value>(&source).map_err(error)?,
                            "exampleLocation":serde_json::from_str::<Value>(&location).map_err(error)?,
                            "sourceSegmentOffset":at,"derivation":derivation,"score":hit.score,"matched":hit.matched});
                        let size = serde_json::to_vec(&item).map_err(error)?.len();
                        if page_bytes.saturating_add(size) <= 1024 * 1024 {
                            page_bytes = page_bytes.saturating_add(size);
                            items.push(item);
                        } else if items.is_empty() {
                            return Err("one located source exceeds the 1 MiB detailed-response allowance".into());
                        } else {
                            page_full = true
                        }
                    }
                    total = total.checked_add(1).ok_or("matching-source count overflow")?;
                }
            }
        }
        let next = offset.saturating_add(items.len());
        Ok(json!({"id":document,"contentFingerprint":content_fp,"metadataFingerprint":metadata_fp,
            "completeFields":complete_fields,"sources":items,"totalSources":total,"offset":offset,
            "nextOffset":(next<total).then_some(next),"scope":"One example location per matching text clause and source, not an occurrence count."}))
    }

    pub fn gaps(&self, account: Option<Id>, offset: usize, limit: usize) -> Result<Value> {
        let account = account.map(|account| account.to_string());
        let total: i64 = self
            .connection
            .query_row(
                "SELECT COUNT(*) FROM metadata m JOIN links l ON l.document=m.id
            JOIN parts p ON p.key=l.part_key WHERE p.complete=0 AND (?1 IS NULL OR m.account=?1)",
                [&account],
                |row| row.get(0),
            )
            .map_err(error)?;
        let total = u64::try_from(total).map_err(error)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT m.id,m.account,m.content_fp,m.metadata_fp,l.source_json,p.state,p.coverage_json
            FROM metadata m JOIN links l ON l.document=m.id JOIN parts p ON p.key=l.part_key
            WHERE p.complete=0 AND (?1 IS NULL OR m.account=?1) ORDER BY m.account,m.id,l.part_key,l.source_json LIMIT ?2 OFFSET ?3",
            )
            .map_err(error)?;
        let mut rows =
            statement.query(params![account, i64::try_from(limit).map_err(error)?, i64::try_from(offset).map_err(error)?]).map_err(error)?;
        let mut items = Vec::new();
        let mut bytes = 0usize;
        while let Some(row) = rows.next().map_err(error)? {
            let coverage: String = row.get(6).map_err(error)?;
            let coverage: Value = serde_json::from_str(&coverage).map_err(error)?;
            let item = json!({"id":row.get::<_,String>(0).map_err(error)?.parse::<Id>().map_err(error)?,
                "account":row.get::<_,String>(1).map_err(error)?.parse::<Id>().map_err(error)?,
                "contentFingerprint":row.get::<_,String>(2).map_err(error)?,"metadataFingerprint":row.get::<_,String>(3).map_err(error)?,
                "source":serde_json::from_str::<Value>(&row.get::<_,String>(4).map_err(error)?).map_err(error)?,"state":row.get::<_,String>(5).map_err(error)?,
                "firstGap":coverage.get("gaps").and_then(Value::as_array).and_then(|gaps| gaps.first()),
                "gapCount":coverage.get("gaps").and_then(Value::as_array).map(Vec::len),
                "sourcePassCount":coverage.get("sources").and_then(Value::as_array).map(Vec::len),"limits":coverage.get("limits")});
            let size = serde_json::to_vec(&item).map_err(error)?.len();
            if bytes.saturating_add(size) > 1024 * 1024 {
                if items.is_empty() {
                    return Err("one coverage gap exceeds the detailed-response allowance".into());
                }
                break;
            }
            bytes = bytes.saturating_add(size);
            items.push(item);
        }
        let next = offset.saturating_add(items.len());
        Ok(json!({"gaps":items,"total":total,"offset":offset,"nextOffset":((next as u64)<total).then_some(next)}))
    }

    pub fn coverage_details(
        &self,
        document: Id,
        attachment: Option<usize>,
        field: Option<&str>,
        section: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Value> {
        if !matches!(section, "gaps" | "sources") {
            return Err("coverage detail section is gaps or sources".into());
        }
        let attachment = attachment.map(i64::try_from).transpose().map_err(error)?;
        let row = self
            .connection
            .query_row(
                "SELECT m.content_fp,m.metadata_fp,p.coverage_json FROM metadata m
            JOIN links l ON l.document=m.id JOIN parts p ON p.key=l.part_key WHERE m.id=?1
            AND ((?2 IS NOT NULL AND json_extract(l.source_json,'$.attachment')=?2)
                OR (?2 IS NULL AND json_extract(l.source_json,'$.field')=?3)) LIMIT 1",
                params![document.to_string(), attachment, field],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(error)?
            .ok_or("the requested source has not been inspected by the persistent index")?;
        let coverage: Value = serde_json::from_str(&row.2).map_err(error)?;
        let records = coverage.get(section).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
        let mut items = Vec::new();
        let mut bytes = 0usize;
        for record in records.iter().skip(offset).take(limit) {
            let size = serde_json::to_vec(record).map_err(error)?.len();
            if bytes.saturating_add(size) > 1024 * 1024 {
                if items.is_empty() {
                    return Err("one native coverage record exceeds the detailed-response allowance".into());
                }
                break;
            }
            bytes = bytes.saturating_add(size);
            items.push(record.clone());
        }
        let next = offset.saturating_add(items.len());
        Ok(json!({"id":document,"contentFingerprint":row.0,"metadataFingerprint":row.1,"section":section,
            "records":items,"total":records.len(),"offset":offset,"nextOffset":(next<records.len()).then_some(next)}))
    }

    pub fn coverage(&self) -> Result<Value> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT account,downloaded,state,COUNT(*),SUM(attachment_count)
            FROM metadata GROUP BY account,downloaded,state",
            )
            .map_err(error)?;
        let mut rows = statement.query([]).map_err(error)?;
        let mut accounts: HashMap<Id, Value> = HashMap::new();
        while let Some(row) = rows.next().map_err(error)? {
            let id: Id = row.get::<_, String>(0).map_err(error)?.parse().map_err(error)?;
            let downloaded: bool = row.get(1).map_err(error)?;
            let state: String = row.get(2).map_err(error)?;
            let count = u64::try_from(row.get::<_, i64>(3).map_err(error)?).map_err(error)?;
            let attachment_count = u64::try_from(row.get::<_, i64>(4).map_err(error)?).map_err(error)?;
            let account = accounts.entry(id).or_insert_with(|| {
                json!({"account":id,"discovered":0,"downloaded":0,"states":{},
                "attachments":{"discovered":0,"downloaded":0,"states":{}}})
            });
            let object = account.as_object_mut().ok_or("invalid account coverage")?;
            let discovered = object.get("discovered").and_then(Value::as_u64).unwrap_or(0).saturating_add(count);
            object.insert("discovered".into(), json!(discovered));
            if downloaded {
                let total = object.get("downloaded").and_then(Value::as_u64).unwrap_or(0).saturating_add(count);
                object.insert("downloaded".into(), json!(total));
            }
            if let Some(states) = object.get_mut("states").and_then(Value::as_object_mut) {
                let total = states.get(&state).and_then(Value::as_u64).unwrap_or(0).saturating_add(count);
                states.insert(state, json!(total));
            }
            if let Some(attachments) = object.get_mut("attachments").and_then(Value::as_object_mut) {
                let total = attachments.get("discovered").and_then(Value::as_u64).unwrap_or(0).saturating_add(attachment_count);
                attachments.insert("discovered".into(), json!(total));
                if downloaded {
                    let total = attachments.get("downloaded").and_then(Value::as_u64).unwrap_or(0).saturating_add(attachment_count);
                    attachments.insert("downloaded".into(), json!(total));
                }
            }
        }
        let mut attachment_statement = self
            .connection
            .prepare(
                "SELECT m.account,p.state,COUNT(*) FROM metadata m
            JOIN links l ON l.document=m.id JOIN parts p ON p.key=l.part_key
            WHERE l.fields=128 GROUP BY m.account,p.state",
            )
            .map_err(error)?;
        let mut rows = attachment_statement.query([]).map_err(error)?;
        while let Some(row) = rows.next().map_err(error)? {
            let id: Id = row.get::<_, String>(0).map_err(error)?.parse().map_err(error)?;
            let state: String = row.get(1).map_err(error)?;
            let count = u64::try_from(row.get::<_, i64>(2).map_err(error)?).map_err(error)?;
            if let Some(states) = accounts
                .get_mut(&id)
                .and_then(|account| account.get_mut("attachments"))
                .and_then(|attachments| attachments.get_mut("states"))
                .and_then(Value::as_object_mut)
            {
                states.insert(state, json!(count));
            }
        }
        for account in accounts.values_mut() {
            if let Some(attachments) = account.get_mut("attachments").and_then(Value::as_object_mut) {
                let accounted = attachments
                    .get("states")
                    .and_then(Value::as_object)
                    .map(|states| states.values().filter_map(Value::as_u64).sum::<u64>())
                    .unwrap_or(0);
                let pending = attachments.get("discovered").and_then(Value::as_u64).unwrap_or(0).saturating_sub(accounted);
                if let Some(states) = attachments.get_mut("states").and_then(Value::as_object_mut) {
                    states.insert("pending".into(), json!(pending));
                }
            }
        }
        let mut accounts: Vec<_> = accounts.into_values().collect();
        accounts.sort_by_key(|row| row.get("account").and_then(Value::as_u64).unwrap_or(0));
        let complete = accounts.iter().all(|account| {
            account
                .get("states")
                .and_then(Value::as_object)
                .is_some_and(|states| states.iter().all(|(state, count)| state == "indexed" || count.as_u64() == Some(0)))
        });
        Ok(json!({"accounts":accounts,"complete":complete,
            "scope":"Every currently discovered retained message and attachment; undiscovered provider history is not inferred.",
            "attachmentInventory":"Complete only for fully downloaded messages; missing bodies can hide additional attachments."}))
    }
}
