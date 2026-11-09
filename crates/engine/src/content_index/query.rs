// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only snapshot queries run away from the UI and native ingestion writer.
//! Cancellation is an actual request signal, never a search-time deadline.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::JoinHandle;

use mailcraft_model::{Id, search::Query};
use serde_json::Value;

use super::db::{Database, MatchFrame};

pub struct Answer {
    pub query: Query,
    pub generation: u64,
    pub frames: HashMap<Id, MatchFrame>,
    pub coverage: Value,
}

pub enum Request {
    Search { query: Query, generation: u64, cancelled: Arc<AtomicBool> },
    Coverage { generation: u64, cancelled: Arc<AtomicBool> },
    Detail { token: u64, request: DetailRequest, generation: u64, cancelled: Arc<AtomicBool> },
}

#[derive(Clone, PartialEq, Eq)]
pub enum DetailRequest {
    Sources { document: Id, query: Query, offset: usize, limit: usize },
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
}

impl Reader {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let (sender, requests) = mpsc::sync_channel::<Request>(1);
        let (events, received) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("mailcraft-content-search".into())
            .spawn(move || {
                while let Ok(request) = requests.recv() {
                    if let Request::Detail { token, request, generation, cancelled } = request {
                        let result = (|| -> Result<Value, String> {
                            let mut database = Database::reader(&path)?;
                            database.cancellation(cancelled.clone())?;
                            database.begin_read()?;
                            let result = match request {
                                DetailRequest::Sources { document, query, offset, limit } => {
                                    database.sources(document, &query, offset, limit, &cancelled)
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
                        Request::Search { query, generation, cancelled } => (generation, cancelled, Some(query)),
                        Request::Coverage { generation, cancelled } => (generation, cancelled, None),
                        Request::Detail { .. } => continue,
                    };
                    let result = (|| -> Result<Event, String> {
                        let mut database = Database::reader(&path)?;
                        database.cancellation(cancelled.clone())?;
                        database.begin_read()?;
                        let result = if let Some(query) = query {
                            if let Some(why) = query.error() {
                                return Err(why.into());
                            }
                            let mut frames = database.frames()?;
                            for index in 0..query.len() {
                                if cancelled.load(Ordering::Acquire) {
                                    return Err("search cancelled".into());
                                }
                                let Some(term) = query.text_of(index) else { continue };
                                let bit = 1u64
                                    .checked_shl(u32::try_from(index).map_err(|why| why.to_string())?)
                                    .ok_or("query exceeds its term-bit allowance")?;
                                for id in database.matching(term, &cancelled)? {
                                    if let Some(frame) = frames.get_mut(&id) {
                                        frame.hits |= bit
                                    }
                                }
                            }
                            Event::Answer(Box::new(Answer { query, generation, frames, coverage: database.coverage()? }))
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
        Ok(Self { sender: Some(sender), events: received, worker: Some(worker), active: false, cancellation: None, completed: None })
    }

    pub fn request(&mut self, query: Option<Query>, generation: u64) -> Result<(), String> {
        if self.active {
            return Err("the original content query is still running".into());
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let request = match query {
            Some(query) => Request::Search { query, generation, cancelled: cancelled.clone() },
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
