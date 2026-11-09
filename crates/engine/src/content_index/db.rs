// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One app-owned writer, read-only WAL readers, and immutable located text.
//! This sidecar is derived data; no operation writes the primary mailbox.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use mailcraft_model::Id;
use mailcraft_model::search::text::{TextTerm, fold_literal, fold_words};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const APPLICATION_ID: i64 = 0x4d435831; // MCX1, never inferred from a filename.
const SCHEMA: i64 = 1;
pub const SEGMENT_BYTES: usize = 256 * 1024;
pub const OVERLAP_BYTES: usize = 64 * 1024;

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
}

#[derive(Clone, Debug)]
pub struct MatchFrame {
    pub content_fp: String,
    pub metadata_fp: String,
    pub complete_fields: u8,
    pub hits: u64,
}

fn error(why: impl std::fmt::Display) -> String {
    why.to_string()
}

impl Database {
    pub fn writer(path: &Path) -> Result<Self> {
        let exists = path.try_exists().map_err(error)?;
        if !exists {
            // Initialize and close an unselected owned file before publishing
            // its name. A crash during schema creation cannot leave a selected
            // zero-identity database which future opens would have to guess at.
            let parent = path.parent().ok_or("the search sidecar has no parent directory")?;
            let temporary = tempfile::NamedTempFile::new_in(parent).map_err(error)?;
            let database = Self::open_file(temporary.path(), true)?;
            database.connection.close().map_err(|(_, why)| error(why))?;
            temporary.persist_noclobber(path).map_err(error)?;
        }
        Self::open_file(path, false)
    }

    fn open_file(path: &Path, fresh: bool) -> Result<Self> {
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
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(error)?;
        let application: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0)).map_err(error)?;
        if !fresh && application != APPLICATION_ID {
            return Err("the existing search-path file is not an owned MailCraft index; it was preserved".into());
        }
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0)).map_err(error)?;
        if !fresh && version != SCHEMA {
            return Err("the search index has another schema version; rebuild it through its owner".into());
        }
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA secure_delete=ON;
            PRAGMA cache_size=-8192; PRAGMA mmap_size=0; PRAGMA temp_store=FILE;
            CREATE TABLE IF NOT EXISTS metadata(
                id TEXT PRIMARY KEY, account TEXT NOT NULL, folder TEXT NOT NULL,
                content_fp TEXT NOT NULL, metadata_fp TEXT NOT NULL,
                row_json TEXT NOT NULL, snippet TEXT NOT NULL,
                downloaded INTEGER NOT NULL, complete_fields INTEGER NOT NULL,
                state TEXT NOT NULL, problem TEXT, attachment_count INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS metadata_folder ON metadata(folder);
            CREATE INDEX IF NOT EXISTS metadata_account ON metadata(account);
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
                raw TEXT NOT NULL, folded TEXT NOT NULL, lexical TEXT NOT NULL,
                UNIQUE(part_key,ordinal));
            CREATE VIRTUAL TABLE IF NOT EXISTS content_fts USING fts5(
                lexical, folded, raw, content='units', content_rowid='id',
                tokenize='trigram case_sensitive 1');",
            )
            .map_err(error)?;
        if fresh {
            connection.pragma_update(None, "application_id", APPLICATION_ID).map_err(error)?;
            connection.pragma_update(None, "user_version", SCHEMA).map_err(error)?;
        }
        Ok(Self { connection })
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
                "PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;
            PRAGMA cache_size=-8192; PRAGMA mmap_size=0; PRAGMA temp_store=FILE;",
            )
            .map_err(error)?;
        Ok(Self { connection })
    }

    pub fn metadata(&mut self, rows: &[Metadata]) -> Result<()> {
        let transaction = self.connection.transaction().map_err(error)?;
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
                    "INSERT INTO metadata VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
                ON CONFLICT(id) DO UPDATE SET account=excluded.account,folder=excluded.folder,
                content_fp=excluded.content_fp,metadata_fp=excluded.metadata_fp,row_json=excluded.row_json,
                snippet=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.snippet ELSE excluded.snippet END,
                downloaded=excluded.downloaded,attachment_count=excluded.attachment_count,
                complete_fields=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.complete_fields ELSE excluded.complete_fields END,
                state=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.state ELSE excluded.state END,
                problem=CASE WHEN metadata.content_fp=excluded.content_fp THEN metadata.problem ELSE excluded.problem END",
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
        let transaction = self.connection.transaction().map_err(error)?;
        for id in ids {
            transaction.execute("DELETE FROM metadata WHERE id=?1", [id.to_string()]).map_err(error)?;
        }
        transaction.commit().map_err(error)
    }

    pub fn retry_failed(&mut self) -> Result<Vec<Id>> {
        let transaction = self.connection.transaction().map_err(error)?;
        let ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT DISTINCT document FROM links JOIN parts ON parts.key=links.part_key
                WHERE parts.state IN ('failed','missing')",
                )
                .map_err(error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(error)?
                .map(|row| row.map_err(error)?.parse::<Id>().map_err(error))
                .collect::<Result<Vec<_>>>()?
        };
        for id in &ids {
            transaction.execute("UPDATE metadata SET state='pending',complete_fields=0,problem=NULL WHERE id=?1", [id.to_string()]).map_err(error)?;
        }
        transaction.execute("DELETE FROM links WHERE part_key IN (SELECT key FROM parts WHERE state IN ('failed','missing'))", []).map_err(error)?;
        transaction.commit().map_err(error)?;
        self.prune()?;
        Ok(ids)
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
            let transaction = self.connection.transaction().map_err(error)?;
            loop {
                let unit = transaction
                    .query_row("SELECT id,lexical,folded,raw FROM units WHERE part_key=?1 ORDER BY id LIMIT 1", [&key], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
                    })
                    .optional()
                    .map_err(error)?;
                let Some((id, lexical, folded, raw)) = unit else { break };
                transaction
                    .execute(
                        "INSERT INTO content_fts(content_fts,rowid,lexical,folded,raw)
                    VALUES('delete',?1,?2,?3,?4)",
                        params![id, lexical, folded, raw],
                    )
                    .map_err(error)?;
                transaction.execute("DELETE FROM units WHERE id=?1", [id]).map_err(error)?;
            }
            transaction.execute("DELETE FROM parts WHERE key=?1", [&key]).map_err(error)?;
            transaction.commit().map_err(error)?;
        }
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
    /// parser/OCR process runs while this short commit holds the writer.
    pub fn part(
        &mut self,
        key: &str,
        state: &str,
        complete: bool,
        coverage: &Value,
        segments: impl IntoIterator<Item = Result<Segment>>,
    ) -> Result<()> {
        let transaction = self.connection.transaction().map_err(error)?;
        // Existing completed content is immutable and source/version-addressed.
        if transaction.query_row("SELECT EXISTS(SELECT 1 FROM parts WHERE key=?1)", [key], |row| row.get::<_, bool>(0)).map_err(error)? {
            return Err("an existing immutable extraction key cannot be overwritten".into());
        }
        transaction.execute("INSERT INTO parts VALUES(?1,?2,?3,?4)", params![key, state, complete, coverage.to_string()]).map_err(error)?;
        for (ordinal, segment) in segments.into_iter().enumerate() {
            let segment = segment?;
            if segment.raw.len() > SEGMENT_BYTES + OVERLAP_BYTES {
                return Err("the extractor returned an oversized text segment".into());
            }
            let offset = i64::try_from(segment.offset).map_err(error)?;
            let ordinal = i64::try_from(ordinal).map_err(error)?;
            let folded = fold_literal(&segment.raw);
            let lexical = fold_words(&segment.raw);
            transaction
                .execute(
                    "INSERT INTO units(part_key,ordinal,location_json,source_offset,
                starts_word,ends_word,derivation,raw,folded,lexical) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        key,
                        ordinal,
                        segment.location.to_string(),
                        offset,
                        segment.starts_word,
                        segment.ends_word,
                        segment.derivation,
                        segment.raw,
                        folded,
                        lexical
                    ],
                )
                .map_err(error)?;
            let id = transaction.last_insert_rowid();
            transaction
                .execute("INSERT INTO content_fts(rowid,lexical,folded,raw) VALUES(?1,?2,?3,?4)", params![id, lexical, folded, segment.raw])
                .map_err(error)?;
        }
        transaction.commit().map_err(error)
    }

    pub fn link(&mut self, links: &[Link]) -> Result<()> {
        let transaction = self.connection.transaction().map_err(error)?;
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

    /// Candidate lookup and exact source verification, streamed without a
    /// result-count cap or whole-document allocations.
    pub fn matching(&self, term: &TextTerm, cancelled: &std::sync::atomic::AtomicBool) -> Result<HashSet<Id>> {
        use std::sync::atomic::Ordering;
        let (column, candidate) = term.candidate();
        let use_fts = candidate.chars().count() >= 3 && !candidate.contains('\0');
        let expression = format!("{column} : \"{}\"", candidate.replace('"', "\"\""));
        let sql = if use_fts {
            "SELECT l.document,u.raw,u.starts_word,u.ends_word,u.part_key FROM content_fts
             JOIN units u ON u.id=content_fts.rowid JOIN links l ON l.part_key=u.part_key
             WHERE content_fts MATCH ?1 AND (l.fields & ?2)<>0 AND l.available=1"
        } else {
            "SELECT l.document,u.raw,u.starts_word,u.ends_word,u.part_key FROM units u
             JOIN links l ON l.part_key=u.part_key WHERE ?1 IS NOT NULL AND (l.fields & ?2)<>0 AND l.available=1"
        };
        let mut statement = self.connection.prepare(sql).map_err(error)?;
        let mut rows = statement.query(params![expression, i64::from(term.fields)]).map_err(error)?;
        let mut matches = HashSet::new();
        let mut checked_parts = HashMap::<String, bool>::new();
        while let Some(row) = rows.next().map_err(error)? {
            if cancelled.load(Ordering::Acquire) {
                return Err("search cancelled".into());
            }
            let id: Id = row.get::<_, String>(0).map_err(error)?.parse().map_err(error)?;
            if !matches.contains(&id) {
                let hit = if !term.literal && term.word_count() > 1 {
                    let key: String = row.get(4).map_err(error)?;
                    match checked_parts.get(&key) {
                        Some(hit) => *hit,
                        None => {
                            let hit = self.matching_words(&key, term, cancelled)?;
                            checked_parts.insert(key, hit);
                            hit
                        }
                    }
                } else {
                    let raw: String = row.get(1).map_err(error)?;
                    term.matches_segment(&raw, row.get(2).map_err(error)?, row.get(3).map_err(error)?)
                };
                if hit {
                    matches.insert(id);
                }
            }
        }
        Ok(matches)
    }

    fn matching_words(&self, key: &str, term: &TextTerm, cancelled: &std::sync::atomic::AtomicBool) -> Result<bool> {
        Ok(self.matching_words_location(key, term, cancelled)?.is_some())
    }

    fn matching_words_location(
        &self,
        key: &str,
        term: &TextTerm,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<(String, u64, String)>> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT location_json,source_offset,raw,derivation FROM units
            WHERE part_key=?1 ORDER BY ordinal",
            )
            .map_err(error)?;
        let mut rows = statement.query([key]).map_err(error)?;
        let mut location = None::<String>;
        let mut next = 0u64;
        let mut last_offset = 0u64;
        let mut last_derivation = String::new();
        let mut stream = term.stream();
        while let Some(row) = rows.next().map_err(error)? {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err("search cancelled".into());
            }
            let unit: String = row.get(0).map_err(error)?;
            let offset = u64::try_from(row.get::<_, i64>(1).map_err(error)?).map_err(error)?;
            let raw: String = row.get(2).map_err(error)?;
            let derivation: String = row.get(3).map_err(error)?;
            if location.as_ref() != Some(&unit) {
                if stream.finish() {
                    return Ok(location.map(|location| (location, last_offset, last_derivation)));
                }
                stream = term.stream();
                next = 0;
                location = Some(unit);
            }
            if offset > next {
                return Err("the indexed text unit contains an unaccounted-for source gap".into());
            }
            let skip = usize::try_from(next.saturating_sub(offset)).map_err(error)?;
            if skip <= raw.len() {
                let text = raw.get(skip..).ok_or("the indexed continuation splits a UTF-8 character")?;
                if stream.push(text) {
                    return Ok(location.map(|location| (location, offset, derivation)));
                }
                last_offset = offset;
                last_derivation = derivation;
                next = offset.checked_add(raw.len() as u64).ok_or("indexed text offset overflow")?;
            }
        }
        Ok(if stream.finish() { location.map(|location| (location, last_offset, last_derivation)) } else { None })
    }

    pub fn sources(
        &self,
        document: Id,
        query: &mailcraft_model::search::Query,
        offset: usize,
        limit: usize,
        cancelled: &std::sync::atomic::AtomicBool,
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
            let mut statement = self
                .connection
                .prepare(
                    "SELECT part_key,source_json FROM links
                WHERE document=?1 AND (fields & ?2)<>0 AND available=1 ORDER BY part_key,source_json",
                )
                .map_err(error)?;
            let mut rows = statement.query(params![document.to_string(), i64::from(term.fields)]).map_err(error)?;
            while let Some(row) = rows.next().map_err(error)? {
                if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                    return Err("search source inspection cancelled".into());
                }
                let key: String = row.get(0).map_err(error)?;
                let source: String = row.get(1).map_err(error)?;
                let found = if !term.literal && term.word_count() > 1 {
                    self.matching_words_location(&key, term, cancelled)?
                } else {
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
                        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                            return Err("search source inspection cancelled".into());
                        }
                        let raw: String = unit.get(3).map_err(error)?;
                        if term.matches_segment(&raw, unit.get(4).map_err(error)?, unit.get(5).map_err(error)?) {
                            found = Some((
                                unit.get::<_, String>(0).map_err(error)?,
                                u64::try_from(unit.get::<_, i64>(1).map_err(error)?).map_err(error)?,
                                unit.get::<_, String>(2).map_err(error)?,
                            ));
                            break;
                        }
                    }
                    found
                };
                if let Some((location, at, derivation)) = found {
                    if total >= offset && items.len() < limit && !page_full {
                        let item = json!({"clause":index,"source":serde_json::from_str::<Value>(&source).map_err(error)?,
                            "exampleLocation":serde_json::from_str::<Value>(&location).map_err(error)?,
                            "sourceSegmentOffset":at,"derivation":derivation});
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
