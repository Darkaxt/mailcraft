// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Persistent full-content search. One writer owns extraction and commits; one
//! read-only worker owns queries. Neither owns a second mailbox or a network sync.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;

use mailcraft_model::{Id, Message, search::Query};
use serde_json::{Value, json};

use crate::Session;

mod db;
mod documents;
#[cfg(windows)]
mod evidence;
mod extract;
mod input;
mod query;
mod segments;
#[cfg(test)]
mod tests;

pub use query::DetailRequest;

struct Detail {
    request: DetailRequest,
    result: Option<Result<Value, String>>,
    generation: u64,
    closing: bool,
}

enum Job {
    Metadata(Vec<db::Metadata>),
    Remove(Vec<Id>),
    Document(Box<input::Input>, documents::Phase),
    Prune,
    Retry,
}

enum Event {
    Ready { revision: String, stored: HashSet<Id> },
    Metadata { needed: Vec<(Id, documents::Phase)>, committed: Vec<Id> },
    Document(documents::Completion),
    Removed,
    Pruned,
    Retried(Vec<Id>),
    Failed(String),
}

struct Chunk {
    pointer: Weak<mailcraft_model::messages::Chunk>,
    ids: Vec<Id>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum QueryOrigin {
    Window,
    Agent,
}

pub struct Index {
    mailbox: PathBuf,
    path: PathBuf,
    sender: Option<mpsc::SyncSender<Job>>,
    events: mpsc::Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    #[cfg(windows)]
    native_cancel: Arc<std::sync::Mutex<Option<craft_content::process::Cancellation>>>,
    active: bool,
    initialized: bool,
    revision: String,
    chunks: HashMap<Id, Chunk>,
    messages: HashMap<Id, Weak<Message>>,
    metadata: VecDeque<Id>,
    metadata_set: HashSet<Id>,
    bodies: VecDeque<Id>,
    body_set: HashSet<Id>,
    attachments: VecDeque<Id>,
    attachment_set: HashSet<Id>,
    removals: HashSet<Id>,
    dirty: HashSet<Id>,
    versions: HashMap<Id, (String, String)>,
    generation: u64,
    presentation: u64,
    pruned: bool,
    pub problem: Option<String>,
    pub query_problem: Option<String>,
    reader: Option<query::Reader>,
    rows: Option<db::Database>,
    answers: VecDeque<Box<query::Answer>>,
    wanted_window: Option<Query>,
    requested: Option<(Query, u64, QueryOrigin)>,
    pending_queries: VecDeque<(Query, QueryOrigin)>,
    coverage: Option<(u64, Value)>,
    zone: String,
    details: HashMap<u64, Detail>,
    detail_queue: VecDeque<u64>,
    detail_running: Option<u64>,
    next_detail: u64,
    closed_details: VecDeque<u64>,
    retry_requested: bool,
}

impl Index {
    pub fn open(mailbox: &Path, render_evidence: Option<PathBuf>) -> Result<Self, String> {
        let path = crate::store::content_index_file(mailbox);
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (events, received) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = cancelled.clone();
        let worker_path = path.clone();
        #[cfg(windows)]
        let native_cancel = Arc::new(std::sync::Mutex::new(None));
        #[cfg(windows)]
        let bind_cancel = native_cancel.clone();
        let worker = std::thread::Builder::new()
            .name("mailcraft-content-ingest".into())
            .spawn(move || {
                let mut database = match db::Database::writer(&worker_path) {
                    Ok(database) => database,
                    Err(why) => {
                        let _ = events.send(Event::Failed(why));
                        return;
                    }
                };
                let result = (|| -> Result<(), String> {
                    let _ = database.retry_failed()?;
                    let extractor = extract::Extractor::new(render_evidence)?;
                    #[cfg(windows)]
                    {
                        *bind_cancel.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(extractor.cancellation.clone());
                        if stop.load(Ordering::Acquire) {
                            extractor.cancellation.cancel()
                        }
                    }
                    events
                        .send(Event::Ready { revision: extractor.revision().into(), stored: database.document_ids()? })
                        .map_err(|why| why.to_string())?;
                    let temporary_root = std::env::temp_dir();
                    while let Ok(job) = jobs.recv() {
                        if stop.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        let event = match job {
                            Job::Metadata(rows) => {
                                let mut needed = Vec::new();
                                for row in &rows {
                                    match database.current(row.id)? {
                                        None => needed.push((row.id, documents::Phase::Body)),
                                        Some((content, _, _, _)) if content != row.content_fp => needed.push((row.id, documents::Phase::Body)),
                                        Some((_, _, _, state)) if state == "pendingAttachments" => {
                                            needed.push((row.id, documents::Phase::Attachments))
                                        }
                                        Some((_, _, _, state)) if state == "pending" => needed.push((row.id, documents::Phase::Body)),
                                        _ => {}
                                    }
                                }
                                database.metadata(&rows)?;
                                Event::Metadata { needed, committed: rows.iter().map(|row| row.id).collect() }
                            }
                            Job::Remove(ids) => {
                                database.remove(ids)?;
                                Event::Removed
                            }
                            Job::Document(input, phase) => {
                                let scratch = tempfile::Builder::new()
                                    .prefix("mailcraft-content-")
                                    .tempdir_in(&temporary_root)
                                    .map_err(|why| why.to_string())?;
                                Event::Document(documents::index(&mut database, *input, scratch.path(), &extractor, &stop, phase)?)
                            }
                            Job::Prune => {
                                database.prune()?;
                                Event::Pruned
                            }
                            Job::Retry => Event::Retried(database.retry_failed()?),
                        };
                        if events.send(event).is_err() {
                            return Ok(());
                        }
                    }
                    Ok(())
                })();
                if let Err(why) = result {
                    let _ = events.send(Event::Failed(why));
                }
                // A cancelled/failed job may finish before reader teardown.
                // Retain this writer until the owner disconnects after joining
                // its readers. Retire pending jobs without replaying them.
                while jobs.recv().is_ok() {}
                drop(database);
            })
            .map_err(|why| why.to_string())?;
        Ok(Self {
            mailbox: mailbox.into(),
            path,
            sender: Some(sender),
            events: received,
            worker: Some(worker),
            cancelled,
            #[cfg(windows)]
            native_cancel,
            active: true,
            initialized: false,
            revision: String::new(),
            chunks: HashMap::new(),
            messages: HashMap::new(),
            metadata: VecDeque::new(),
            metadata_set: HashSet::new(),
            bodies: VecDeque::new(),
            body_set: HashSet::new(),
            attachments: VecDeque::new(),
            attachment_set: HashSet::new(),
            removals: HashSet::new(),
            dirty: HashSet::new(),
            versions: HashMap::new(),
            generation: 0,
            presentation: 0,
            pruned: true,
            problem: None,
            query_problem: None,
            reader: None,
            rows: None,
            answers: VecDeque::new(),
            wanted_window: None,
            requested: None,
            pending_queries: VecDeque::new(),
            coverage: None,
            zone: String::new(),
            details: HashMap::new(),
            detail_queue: VecDeque::new(),
            detail_running: None,
            next_detail: 0,
            closed_details: VecDeque::new(),
            retry_requested: false,
        })
    }

    pub fn mailbox(&self) -> &Path {
        &self.mailbox
    }
    pub fn finished(&self) -> u64 {
        self.presentation
    }
    pub fn building(&self) -> bool {
        self.problem.is_none()
            && (!self.initialized
                || self.active
                || !self.metadata_set.is_empty()
                || !self.body_set.is_empty()
                || !self.attachment_set.is_empty()
                || !self.removals.is_empty()
                || !self.pruned
                || self.retry_requested)
    }

    pub fn busy(&self) -> bool {
        self.building()
            || self.reader.as_ref().is_some_and(|reader| reader.active)
            || !self.pending_queries.is_empty()
            || !self.detail_queue.is_empty()
    }

    fn enqueue_metadata(&mut self, id: Id) {
        if self.metadata_set.insert(id) {
            self.metadata.push_back(id)
        }
        self.dirty.insert(id);
        self.pruned = false;
    }

    fn enqueue(&mut self, id: Id, phase: documents::Phase) {
        match phase {
            documents::Phase::Body if self.body_set.insert(id) => self.bodies.push_back(id),
            documents::Phase::Attachments if self.attachment_set.insert(id) => self.attachments.push_back(id),
            _ => {}
        }
        self.pruned = false;
    }

    fn event(&mut self, event: Event, session: &Session) {
        self.active = false;
        self.generation = self.generation.saturating_add(1);
        self.presentation = self.presentation.saturating_add(1);
        match event {
            Event::Ready { revision, stored } => {
                self.initialized = true;
                self.revision = revision;
                self.removals.extend(stored.into_iter().filter(|id| !session.mailbox.messages.contains_key(id)));
                match (query::Reader::new(self.path.clone()), db::Database::reader(&self.path)) {
                    (Ok(reader), Ok(rows)) => {
                        self.reader = Some(reader);
                        self.rows = Some(rows)
                    }
                    (Err(why), _) | (_, Err(why)) => self.problem = Some(why),
                }
            }
            Event::Metadata { needed, committed } => {
                for id in committed {
                    if !self.metadata_set.contains(&id)
                        && session
                            .mailbox
                            .messages
                            .get(&id)
                            .is_some_and(|message| self.messages.get(&id).is_some_and(|known| known.ptr_eq(&Arc::downgrade(message))))
                    {
                        self.dirty.remove(&id);
                    }
                }
                for (id, phase) in needed {
                    self.enqueue(id, phase)
                }
            }
            Event::Document(done) => {
                if self.versions.get(&done.id).is_some_and(|(fingerprint, _)| fingerprint == &done.fingerprint) && done.needs_attachments {
                    self.enqueue(done.id, documents::Phase::Attachments)
                }
            }
            Event::Retried(ids) => {
                for id in ids {
                    self.enqueue(id, documents::Phase::Body)
                }
            }
            Event::Pruned => self.pruned = true,
            Event::Failed(why) => self.problem = Some(why),
            Event::Removed => {}
        }
    }

    fn reconcile(&mut self, session: &Session) {
        let zone = session.zone_key();
        let changed_zone = self.zone != zone;
        self.zone = zone;
        let now = session.mailbox.messages.chunks();
        let gone: Vec<_> = self.chunks.keys().filter(|key| !now.contains_key(key)).copied().collect();
        for key in gone {
            if let Some(chunk) = self.chunks.remove(&key) {
                for id in chunk.ids {
                    self.remove(id)
                }
            }
        }
        for (&key, chunk) in now {
            let pointer = Arc::downgrade(chunk);
            if !changed_zone && self.chunks.get(&key).is_some_and(|seen| seen.pointer.ptr_eq(&pointer)) {
                continue;
            }
            if let Some(previous) = self.chunks.get(&key) {
                let removed: Vec<_> = previous.ids.iter().filter(|id| !chunk.contains_key(id)).copied().collect();
                for id in removed {
                    self.remove(id)
                }
            }
            for (&id, message) in chunk.iter() {
                let pointer = Arc::downgrade(message);
                if changed_zone || !self.messages.get(&id).is_some_and(|previous| previous.ptr_eq(&pointer)) {
                    self.messages.insert(id, pointer);
                    self.enqueue_metadata(id);
                }
            }
            self.chunks.insert(key, Chunk { pointer, ids: chunk.keys().copied().collect() });
        }
    }

    fn remove(&mut self, id: Id) {
        self.messages.remove(&id);
        self.versions.remove(&id);
        self.dirty.remove(&id);
        self.metadata_set.remove(&id);
        self.body_set.remove(&id);
        self.attachment_set.remove(&id);
        self.removals.insert(id);
        self.pruned = false;
    }

    fn send(&mut self, job: Job) {
        match self
            .sender
            .as_ref()
            .ok_or_else(|| "the original content writer is closed".to_owned())
            .and_then(|sender| sender.send(job).map_err(|why| why.to_string()))
        {
            Ok(()) => self.active = true,
            Err(why) => self.problem = Some(why),
        }
    }

    pub fn poll(&mut self, session: &Session) {
        loop {
            match self.events.try_recv() {
                Ok(event) => self.event(event, session),
                Err(mpsc::TryRecvError::Disconnected) if self.active && !self.cancelled.load(Ordering::Acquire) => {
                    self.active = false;
                    self.problem = Some("the original ingestion worker exited without its completion result".into());
                    break;
                }
                _ => break,
            }
        }
        self.reconcile(session);
        if self.initialized && !self.active && self.problem.is_none() {
            if !self.removals.is_empty() {
                let removed = self.removals.drain().collect();
                self.send(Job::Remove(removed));
            } else if self.retry_requested {
                self.retry_requested = false;
                self.send(Job::Retry);
            } else if !self.metadata_set.is_empty() {
                let mut rows = Vec::new();
                let mut batch_bytes = 0usize;
                while rows.len() < 64 {
                    let Some(id) = self.metadata.pop_front() else { break };
                    if !self.metadata_set.remove(&id) {
                        continue;
                    }
                    let Some(message) = session.mailbox.messages.get(&id) else { continue };
                    match input::metadata(session, message, &self.revision) {
                        Ok(row) => {
                            let bytes = input::metadata_bytes(&row);
                            if !rows.is_empty() && batch_bytes.saturating_add(bytes) > 1024 * 1024 {
                                self.metadata.push_front(id);
                                self.metadata_set.insert(id);
                                break;
                            }
                            batch_bytes = batch_bytes.saturating_add(bytes);
                            self.versions.insert(id, (row.content_fp.clone(), row.metadata_fp.clone()));
                            rows.push(row);
                            if batch_bytes >= 1024 * 1024 {
                                break;
                            }
                        }
                        Err(why) => {
                            self.problem = Some(why);
                            break;
                        }
                    }
                }
                if !rows.is_empty() {
                    self.send(Job::Metadata(rows))
                }
            } else {
                let next = loop {
                    if let Some(id) = self.bodies.pop_front() {
                        if self.body_set.remove(&id) {
                            break Some((id, documents::Phase::Body));
                        }
                    } else if let Some(id) = self.attachments.pop_front() {
                        if self.attachment_set.remove(&id) {
                            break Some((id, documents::Phase::Attachments));
                        }
                    } else {
                        break None;
                    }
                };
                if let Some((id, phase)) = next {
                    if session.mailbox.messages.contains_key(&id) {
                        match input::Input::new(session, id, &self.revision) {
                            Ok(input) => {
                                if self.versions.get(&id) != Some(&(input.metadata.content_fp.clone(), input.metadata.metadata_fp.clone())) {
                                    self.enqueue_metadata(id);
                                    self.enqueue(id, phase);
                                } else {
                                    self.send(Job::Document(Box::new(input), phase))
                                }
                            }
                            Err(why) => self.problem = Some(why),
                        }
                    }
                } else if !self.pruned {
                    self.send(Job::Prune)
                }
            }
        }
        self.poll_queries();
    }

    fn poll_queries(&mut self) {
        if let Some(reader) = &mut self.reader {
            while let Some(event) = reader.poll() {
                match event {
                    query::Event::Answer(answer) => {
                        self.coverage = Some((answer.generation, answer.coverage.clone()));
                        self.answers.retain(|old| old.query != answer.query);
                        self.answers.push_front(answer);
                        while self.answers.len() > 4 {
                            self.answers.pop_back();
                        }
                        self.presentation = self.presentation.saturating_add(1);
                    }
                    query::Event::Coverage { generation, coverage } => {
                        self.coverage = Some((generation, coverage));
                        self.presentation = self.presentation.saturating_add(1);
                    }
                    query::Event::Failed(why) => {
                        if let Some(token) = self.detail_running.take()
                            && let Some(detail) = self.details.get_mut(&token)
                        {
                            detail.result = Some(Err(why.clone()))
                        }
                        self.problem = Some(why);
                    }
                    query::Event::Cancelled => {}
                    query::Event::Detail { token, generation, result } => {
                        self.detail_running = None;
                        if self.details.get(&token).is_some_and(|detail| detail.closing) {
                            self.details.remove(&token);
                            self.closed_details.push_back(token);
                            while self.closed_details.len() > 32 {
                                self.closed_details.pop_front();
                            }
                        } else if let Some(detail) = self.details.get_mut(&token) {
                            detail.generation = generation;
                            detail.result = Some(result);
                        }
                        self.presentation = self.presentation.saturating_add(1);
                    }
                }
                self.requested = None;
            }
            if !reader.active && self.problem.is_none() {
                while self.detail_queue.front().is_some_and(|token| !self.details.contains_key(token)) {
                    self.detail_queue.pop_front();
                }
                if let Some(token) = self.detail_queue.pop_front()
                    && let Some(detail) = self.details.get(&token)
                {
                    match reader.detail(token, detail.request.clone(), self.generation) {
                        Ok(()) => self.detail_running = Some(token),
                        Err(why) => self.problem = Some(why),
                    }
                    return;
                }
                while self
                    .pending_queries
                    .front()
                    .is_some_and(|(query, _)| self.answers.iter().any(|answer| &answer.query == query && answer.generation == self.generation))
                {
                    self.pending_queries.pop_front();
                }
                if let Some((query, origin)) = self.pending_queries.pop_front() {
                    let generation = self.generation;
                    match reader.request(Some(query.clone()), generation) {
                        Ok(()) => self.requested = Some((query, generation, origin)),
                        Err(why) => self.problem = Some(why),
                    }
                } else if self.coverage.as_ref().is_none_or(|(generation, _)| *generation != self.generation)
                    && let Err(why) = reader.request(None, self.generation)
                {
                    self.problem = Some(why)
                }
            }
        }
    }

    pub fn query(&mut self, query: Query, origin: QueryOrigin) {
        self.query_problem = None;
        if origin == QueryOrigin::Window && self.wanted_window.as_ref() != Some(&query) {
            self.pending_queries.retain(|(_, origin)| *origin != QueryOrigin::Window);
            if self.requested.as_ref().is_some_and(|(running, _, origin)| *origin == QueryOrigin::Window && running != &query)
                && let Some(reader) = &self.reader
            {
                reader.cancel()
            }
            self.wanted_window = Some(query.clone());
        }
        if query.is_empty() || query.error().is_some() || !(0..query.len()).any(|index| query.text_of(index).is_some()) {
            self.poll_queries();
            return;
        }
        if let Some((running, _, owner)) = &mut self.requested
            && running == &query
        {
            // An agent can share the already running read. Later window edits
            // may cancel a window-only read, never that agent's original work.
            if origin == QueryOrigin::Agent {
                *owner = origin
            }
            return;
        }
        if let Some((_, owner)) = self.pending_queries.iter_mut().find(|(pending, _)| pending == &query) {
            if origin == QueryOrigin::Agent {
                *owner = origin
            }
        } else if !self.answers.iter().any(|answer| answer.query == query && answer.generation == self.generation) {
            if self.pending_queries.len() >= 8 {
                self.query_problem = Some("eight original content queries are already queued; finish them before starting another".into());
            } else {
                self.pending_queries.push_back((query, origin))
            }
        }
        self.poll_queries();
    }

    pub fn decisions(&self, session: &Session, query: &Query) -> (HashMap<Id, bool>, usize) {
        if query.is_empty() || !(0..query.len()).any(|index| query.text_of(index).is_some()) {
            return (HashMap::new(), 0);
        }
        let answer = self.answers.iter().find(|answer| &answer.query == query);
        let mut decisions = HashMap::new();
        let mut unknown = 0usize;
        let in_view = crate::list::in_view(session);
        for message in session.mailbox.messages.values() {
            if !in_view(message) {
                continue;
            }
            let frame = answer.and_then(|answer| answer.frames.get(&message.id)).filter(|frame| {
                !self.dirty.contains(&message.id)
                    && self
                        .versions
                        .get(&message.id)
                        .is_some_and(|(content, metadata)| *content == frame.content_fp && *metadata == frame.metadata_fp)
            });
            let mut missing = false;
            let hit = frame.is_some_and(|frame| {
                for index in 0..query.len() {
                    if let Some(term) = query.text_of(index) {
                        let bit = 1u64.checked_shl(index as u32).unwrap_or(0);
                        if frame.hits & bit == 0 && frame.complete_fields & term.fields != term.fields {
                            missing = true;
                            if query.term_negated(index) {
                                return false;
                            }
                        }
                    }
                }
                query.matches_with(
                    message,
                    |at| session.offset_at(at),
                    |index| query.text_of(index).map(|_| frame.hits & 1u64.checked_shl(index as u32).unwrap_or(0) != 0),
                )
            });
            unknown += usize::from(frame.is_none() || missing);
            decisions.insert(message.id, hit);
        }
        (decisions, unknown)
    }

    pub fn row(&self, message: &Message) -> Option<Value> {
        if self.dirty.contains(&message.id) || !self.messages.get(&message.id).is_some_and(|known| std::ptr::eq(known.as_ptr(), message)) {
            return None;
        }
        let row = self.rows.as_ref()?;
        let (content, metadata, _, _) = row.current(message.id).ok()??;
        if self.versions.get(&message.id) != Some(&(content, metadata)) {
            return None;
        }
        row.row(message.id).ok().flatten()
    }

    pub fn status(&self) -> Value {
        json!({"engine":"SQLite FTS5","version":1,"initialized":self.initialized,"building":self.building(),
            "generation":self.generation,"metadataPending":self.metadata_set.len(),"bodiesPending":self.body_set.len(),
            "attachmentsPending":self.attachment_set.len(),"activeWriter":self.active,
            "queryPending":self.reader.as_ref().is_some_and(|reader| reader.active),
            "coverageGeneration":self.coverage.as_ref().map(|(generation,_)| generation),
            "coverageCurrent":self.dirty.is_empty() && self.metadata_set.is_empty() && self.removals.is_empty()
                && self.coverage.as_ref().is_some_and(|(generation,_)| *generation==self.generation),
            "complete":self.complete(),
            "coverage":self.coverage.as_ref().map(|(_,coverage)| coverage),"problem":self.problem,"queryProblem":self.query_problem})
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        if !self.building() {
            return None;
        }
        let indexed = self
            .coverage
            .as_ref()
            .and_then(|(_, coverage)| coverage.get("accounts"))
            .and_then(Value::as_array)
            .map(|accounts| accounts.iter().filter_map(|account| account.get("states")?.get("indexed")?.as_u64()).sum::<u64>())
            .unwrap_or(0);
        Some((usize::try_from(indexed).unwrap_or(usize::MAX).saturating_sub(self.dirty.len()).min(self.messages.len()), self.messages.len()))
    }

    pub fn query_pending(&self, query: &Query) -> bool {
        if query.is_empty() || !(0..query.len()).any(|index| query.text_of(index).is_some()) {
            return false;
        }
        !self.answers.iter().any(|answer| &answer.query == query)
            || self.pending_queries.iter().any(|(pending, _)| pending == query)
            || self.requested.as_ref().is_some_and(|(requested, _, _)| requested == query)
    }

    pub fn memory_profile(&self) -> Value {
        json!({"loaded":self.initialized,"engine":"SQLite FTS5","messages":self.messages.len(),
            "maximumQueuedBodyReferences":0,"maximumActiveMessageReferences":1,
            "cachedQuerySnapshots":self.answers.len(),"sqliteCacheBytesPerConnection":8*1024*1024,
            "sourceBufferBytes":db::SEGMENT_BYTES+db::OVERLAP_BYTES,"bodyLoadsForListPreviews":0})
    }

    pub fn complete(&self) -> bool {
        !self.building()
            && self.problem.is_none()
            && self.coverage.as_ref().is_some_and(|(generation, coverage)| {
                *generation == self.generation && coverage.get("complete").and_then(Value::as_bool) == Some(true)
            })
    }

    pub fn detail(&mut self, request: DetailRequest, token: Option<u64>, close: bool) -> Result<Value, String> {
        let token = if let Some(token) = token {
            if close && self.closed_details.contains(&token) {
                return Ok(json!({"token":token,"state":"closed"}));
            }
            let detail = self.details.get(&token).ok_or("the original content-inspection token is no longer open")?;
            if !detail.request.same_scope(&request) || (!close && detail.request != request) {
                return Err("this token belongs to another original content-inspection request".into());
            }
            token
        } else {
            if close {
                return Err("name the original inspection token to close it".into());
            }
            if self.details.len() >= 4 {
                return Err("close a completed content inspection before opening a fifth".into());
            }
            self.next_detail = self.next_detail.checked_add(1).ok_or("content inspection identity overflow")?;
            let token = self.next_detail;
            self.details.insert(token, Detail { request, result: None, generation: self.generation, closing: false });
            self.detail_queue.push_back(token);
            token
        };
        if close {
            if self.detail_running == Some(token) {
                if let Some(detail) = self.details.get_mut(&token) {
                    detail.closing = true
                }
                if let Some(reader) = &self.reader {
                    reader.cancel()
                }
                return Ok(json!({"token":token,"state":"closing"}));
            }
            self.details.remove(&token);
            self.detail_queue.retain(|queued| *queued != token);
            self.closed_details.push_back(token);
            while self.closed_details.len() > 32 {
                self.closed_details.pop_front();
            }
            return Ok(json!({"token":token,"state":"closed"}));
        }
        self.poll_queries();
        let detail = self.details.get(&token).ok_or("the original inspection result is unavailable")?;
        Ok(match &detail.result {
            None => json!({"token":token,"state":"pending"}),
            Some(Err(why)) => json!({"token":token,"state":"failed","error":why}),
            Some(Ok(result)) => {
                let current = |row: &Value| {
                    row.get("id").and_then(Value::as_u64).is_none_or(|id| {
                        !self.dirty.contains(&id)
                            && self.versions.get(&id).is_some_and(|(content, metadata)| {
                                row.get("contentFingerprint").and_then(Value::as_str) == Some(content.as_str())
                                    && row.get("metadataFingerprint").and_then(Value::as_str) == Some(metadata.as_str())
                            })
                    })
                };
                let current = current(result) && result.get("gaps").and_then(Value::as_array).is_none_or(|rows| rows.iter().all(current));
                json!({"token":token,"state":if current { "ready" } else { "stale" },"snapshotGeneration":detail.generation,
                    "result":if current { Some(result) } else { None },"changedSinceRequest":!current})
            }
        })
    }

    pub fn retry(&mut self) -> Result<Value, String> {
        if let Some(why) = &self.problem {
            return Err(why.clone());
        }
        self.retry_requested = true;
        self.pruned = false;
        Ok(
            json!({"state":"queued","generation":self.generation,"scope":"failed or missing derived index parts; mailbox messages and provider state are unchanged"}),
        )
    }

    pub fn wait(&mut self, session: &Session) {
        loop {
            self.poll(session);
            if self.problem.is_some() {
                return;
            }
            if !self.building() {
                self.query(session.search_query(), QueryOrigin::Agent);
            }
            if self.active {
                match self.events.recv() {
                    Ok(event) => self.event(event, session),
                    Err(why) => {
                        self.problem = Some(why.to_string());
                        return;
                    }
                }
            } else if let Some(reader) = &mut self.reader
                && reader.active
            {
                reader.wait();
            } else if !self.building() {
                return;
            }
        }
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        #[cfg(windows)]
        if let Some(cancellation) = self.native_cancel.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            cancellation.cancel();
        }
        self.reader.take();
        // The writer must be the final SQLite consumer so its normal close can
        // checkpoint WAL. Retaining this read-only preview connection until
        // after writer.join leaves the sidecars owned by the wrong last closer.
        self.rows.take();
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
