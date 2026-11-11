// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only snapshot queries run away from the UI and native ingestion writer.
//! Cancellation is an actual request signal, never a search-time deadline.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mailcraft_model::{Id, search::Query};
use serde_json::Value;

use super::db::{Database, MatchFrame};
use super::matcher::{EXPANSION_CAP, Matcher, group, pieces};
use crate::SearchOptions;

/// While the index is still building, a cached vocabulary is reused this long
/// before it is read again.
const VOCABULARY_AGE: Duration = Duration::from_secs(30);

pub struct Answer {
    pub query: Query,
    pub options: SearchOptions,
    pub generation: u64,
    /// Terms answered approximately (too broad), for the status line.
    pub notes: Vec<String>,
    pub frames: HashMap<Id, MatchFrame>,
    pub coverage: Value,
}

pub enum Request {
    Search { query: Query, options: SearchOptions, generation: u64, building: bool, cancelled: Arc<AtomicBool> },
    Coverage { generation: u64, cancelled: Arc<AtomicBool> },
    Detail { token: u64, request: DetailRequest, generation: u64, cancelled: Arc<AtomicBool> },
}

#[derive(Clone, PartialEq, Eq)]
pub enum DetailRequest {
    Sources { document: Id, query: Query, options: SearchOptions, offset: usize, limit: usize },
    Gaps { account: Option<Id>, offset: usize, limit: usize },
    Coverage { document: Id, attachment: Option<usize>, field: Option<String>, section: String, offset: usize, limit: usize },
}

impl DetailRequest {
    pub fn same_scope(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Sources { document: a, .. }, Self::Sources { document: b, .. }) => a == b,
            (Self::Gaps { account: a, .. }, Self::Gaps { account: b, .. }) => a == b,
            (Self::Coverage { document: a, attachment: aa, field: af, .. }, Self::Coverage { document: b, attachment: ba, field: bf, .. }) => {
                a == b && aa == ba && af == bf
            }
            _ => false,
        }
    }
}

pub enum Event {
    Answer(Box<Answer>),
    Coverage { generation: u64, coverage: Value },
    Failed(String),
    Cancelled,
    Detail { token: u64, generation: u64, result: Result<Value, String> },
}

pub struct Reader {
    sender: Option<mpsc::SyncSender<Request>>,
    events: mpsc::Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    pub active: bool,
    cancellation: Option<Arc<AtomicBool>>,
    completed: Option<Event>,
    /// Units verified by the running search, and how many a scan may read.
    pub progress: Arc<(AtomicU64, AtomicU64)>,
}

/// The vocabulary (token, units holding it) of one index generation.
struct Vocabulary {
    generation: u64,
    loaded: Instant,
    tokens: Vec<(Box<str>, u32)>,
}

/// Stage 1 for one term: the FTS5 candidate expression (pieces ANDed), or
/// `None` for a bounded scan (a regex without literal text). The inner
/// `Err(())`: some piece is in no token, so the term matches nothing.
fn expression(
    source: &str,
    regex: bool,
    vocabulary: &[(Box<str>, u32)],
    expansions: &mut HashMap<String, (Vec<String>, bool)>,
    notes: &mut Vec<String>,
    cancelled: &AtomicBool,
) -> Result<Result<Option<String>, ()>, String> {
    let mut groups = Vec::new();
    for piece in pieces(source, regex) {
        if !expansions.contains_key(&piece) {
            let mut found: Vec<(&str, u32)> = Vec::new();
            for (index, (token, documents)) in vocabulary.iter().enumerate() {
                if index % 8192 == 0 && cancelled.load(Ordering::Acquire) {
                    return Err("search cancelled".into());
                }
                if token.contains(piece.as_str()) {
                    found.push((token, *documents));
                }
            }
            let too_broad = found.len() > EXPANSION_CAP;
            if too_broad {
                found.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
                found.truncate(EXPANSION_CAP);
            }
            let tokens = found.into_iter().map(|(token, _)| token.to_owned()).collect();
            expansions.insert(piece.clone(), (tokens, too_broad));
        }
        let Some((tokens, too_broad)) = expansions.get(&piece) else { continue };
        if *too_broad {
            notes.push(format!("\"{piece}\" is very common: matched through its {EXPANSION_CAP} most frequent words and words starting with it"));
        }
        match group(&piece, tokens, *too_broad) {
            Some(group) => groups.push(group),
            None => return Ok(Err(())),
        }
    }
    Ok(Ok((!groups.is_empty()).then(|| groups.join(" AND "))))
}

impl Reader {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let (sender, requests) = mpsc::sync_channel::<Request>(1);
        let (events, received) = mpsc::channel();
        let progress = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
        let shared = progress.clone();
        let worker = std::thread::Builder::new()
            .name("mailcraft-content-search".into())
            .spawn(move || {
                let progress = shared;
                let mut vocabulary: Option<Vocabulary> = None;
                while let Ok(request) = requests.recv() {
                    if let Request::Detail { token, request, generation, cancelled } = request {
                        let result = (|| -> Result<Value, String> {
                            let mut database = Database::reader(&path)?;
                            database.cancellation(cancelled.clone())?;
                            database.begin_read()?;
                            let result = match request {
                                DetailRequest::Sources { document, query, options, offset, limit } => {
                                    database.sources(document, &query, &options, offset, limit, &cancelled)
                                }
                                DetailRequest::Gaps { account, offset, limit } => database.gaps(account, offset, limit),
                                DetailRequest::Coverage { document, attachment, field, section, offset, limit } => {
                                    database.coverage_details(document, attachment, field.as_deref(), &section, offset, limit)
                                }
                            };
                            database.end_read()?;
                            result
                        })();
                        if events.send(Event::Detail { token, generation, result }).is_err() {
                            return;
                        }
                        continue;
                    }
                    let (generation, cancelled, query) = match request {
                        Request::Search { query, options, generation, building, cancelled } => {
                            (generation, cancelled, Some((query, options, building)))
                        }
                        Request::Coverage { generation, cancelled } => (generation, cancelled, None),
                        Request::Detail { .. } => continue,
                    };
                    let result = (|| -> Result<Event, String> {
                        let mut database = Database::reader(&path)?;
                        database.cancellation(cancelled.clone())?;
                        database.begin_read()?;
                        let result = if let Some((query, options, building)) = query {
                            if let Some(why) = query.error() {
                                return Err(why.into());
                            }
                            progress.0.store(0, Ordering::Relaxed);
                            progress.1.store(0, Ordering::Relaxed);
                            let stale = match &vocabulary {
                                None => true,
                                Some(known) => known.generation != generation && (!building || known.loaded.elapsed() >= VOCABULARY_AGE),
                            };
                            if stale {
                                vocabulary = None;
                                let tokens = database.vocabulary(&cancelled)?;
                                vocabulary = Some(Vocabulary { generation, loaded: Instant::now(), tokens });
                            }
                            let tokens: &[(Box<str>, u32)] = vocabulary.as_ref().map_or(&[][..], |known| known.tokens.as_slice());
                            let mut frames = database.frames()?;
                            let mut expansions = HashMap::new();
                            let mut notes = Vec::new();
                            for index in 0..query.len() {
                                if cancelled.load(Ordering::Acquire) {
                                    return Err("search cancelled".into());
                                }
                                let Some(term) = query.text_of(index) else { continue };
                                let fields = term.fields & options.fields();
                                if fields == 0 {
                                    continue;
                                }
                                let bit = 1u64
                                    .checked_shl(u32::try_from(index).map_err(|why| why.to_string())?)
                                    .ok_or("query exceeds its term-bit allowance")?;
                                let matcher = Matcher::new(&term.source, term.case_sensitive, &options)?;
                                let Ok(candidates) = expression(&term.source, options.regex, tokens, &mut expansions, &mut notes, &cancelled)? else {
                                    continue;
                                };
                                if candidates.is_none() {
                                    progress.1.fetch_add(database.unit_count()?, Ordering::Relaxed);
                                }
                                for (id, hit) in database.matching(&matcher, candidates.as_deref(), fields, &cancelled, &progress.0)? {
                                    if let Some(frame) = frames.get_mut(&id) {
                                        frame.hits |= bit;
                                        frame.terms.push((index, hit));
                                    }
                                }
                            }
                            notes.dedup();
                            Event::Answer(Box::new(Answer { query, options, generation, notes, frames, coverage: database.coverage()? }))
                        } else {
                            Event::Coverage { generation, coverage: database.coverage()? }
                        };
                        database.end_read()?;
                        Ok(result)
                    })();
                    let event = match result {
                        Ok(event) => event,
                        Err(_) if cancelled.load(Ordering::Acquire) => Event::Cancelled,
                        Err(why) => Event::Failed(why),
                    };
                    if events.send(event).is_err() {
                        return;
                    }
                }
            })
            .map_err(|why| why.to_string())?;
        Ok(Self { sender: Some(sender), events: received, worker: Some(worker), active: false, cancellation: None, completed: None, progress })
    }

    pub fn request(&mut self, query: Option<(Query, SearchOptions)>, generation: u64, building: bool) -> Result<(), String> {
        if self.active {
            return Err("the original content query is still running".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = match query {
            Some((query, options)) => Request::Search { query, options, generation, building, cancelled: cancelled.clone() },
            None => Request::Coverage { generation, cancelled: cancelled.clone() },
        };
        self.sender.as_ref().ok_or("the content reader is closed")?.send(request).map_err(|why| why.to_string())?;
        self.cancellation = Some(cancelled);
        self.active = true;
        Ok(())
    }

    pub fn cancel(&self) {
        if let Some(cancelled) = &self.cancellation {
            cancelled.store(true, Ordering::Release)
        }
    }

    pub fn detail(&mut self, token: u64, request: DetailRequest, generation: u64) -> Result<(), String> {
        if self.active {
            return Err("the original read-only content query is still running".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.sender
            .as_ref()
            .ok_or("the content reader is closed")?
            .send(Request::Detail { token, request, generation, cancelled: cancelled.clone() })
            .map_err(|why| why.to_string())?;
        self.cancellation = Some(cancelled);
        self.active = true;
        Ok(())
    }

    pub fn poll(&mut self) -> Option<Event> {
        if let Some(event) = self.completed.take() {
            self.active = false;
            self.cancellation = None;
            return Some(event);
        }
        match self.events.try_recv() {
            Ok(event) => {
                self.active = false;
                self.cancellation = None;
                Some(event)
            }
            Err(mpsc::TryRecvError::Disconnected) if self.active => {
                self.active = false;
                Some(Event::Failed("the original content-reader worker exited without its result".into()))
            }
            _ => None,
        }
    }

    /// Block only on the original worker's completion event. The next poll
    /// consumes that same event through the normal controller transition.
    pub fn wait(&mut self) {
        if self.active && self.completed.is_none() {
            self.completed =
                Some(self.events.recv().unwrap_or_else(|_| Event::Failed("the original content-reader worker exited without its result".into())));
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.cancel();
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
