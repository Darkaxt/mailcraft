// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Owned native source/export snapshots: one producer and bounded byte pages.
//! Temporary snapshots never replace mailbox data or expose a host path.

use crate::{CmdResult, Session};

/// A decoded body for inspection, or a complete reconstructed MIME message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Body,
    Eml,
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
#[path = "source_exports_tests.rs"]
mod tests;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::Exports;

impl Session {
    /// Deterministic native worker completion for synthetic test harnesses.
    #[cfg(all(any(test, feature = "test-support"), not(target_arch = "wasm32")))]
    pub fn wait_source_exports(&mut self) {
        self.source_exports.wait();
        self.poll_source_exports();
    }

    /// Drain actual producer completions. Hosts poll while a native job is busy;
    /// no timeout stops a producer or turns partial output into a complete file.
    pub fn poll_source_exports(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.source_exports.bind(self.openings());
            self.source_exports.poll();
            self.source_exports.prune(&self.mailbox);
        }
    }

    pub fn source_export_busy(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.source_exports.busy()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// Engine dispatch for a specific message; opaque tokens remain bound to it.
    pub(crate) fn source_export(&mut self, id: u64, kind: Kind, token: Option<u64>, offset: u64, limit: usize, close: bool) -> CmdResult {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.poll_source_exports();
            if let Some(token) = token {
                return self.source_exports.reply(id, kind, token, offset, limit, close);
            }
            if close || offset != 0 {
                return Err(crate::cmd::bad("open the source before reading or closing its token"));
            }
            let input = native::Input::new(self, id, kind)?;
            self.source_exports.begin(input)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (id, kind, token, offset, limit, close);
            Err(crate::cmd::fail(
                "streaming source snapshots require the native app; View Source and Save Message As remain available in the browser",
            ))
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn export_eml_file(&mut self, id: u64, token: Option<u64>, path: &std::path::Path) -> CmdResult {
        self.poll_source_exports();
        let mut snapshot = if let Some(token) = token {
            self.source_exports.take_eml(id, token)?
        } else {
            let input = native::Input::new(self, id, Kind::Eml)?;
            input.produce(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))).map_err(|why| crate::cmd::fail(why.to_string()))?
        };
        let bytes = crate::mark::write_stream_from_mail(path, &mut snapshot.file, snapshot.len).map_err(|why| crate::cmd::fail(why.to_string()))?;
        if bytes != snapshot.len {
            return Err(crate::cmd::fail("the export did not copy the complete prepared message"));
        }
        Ok(serde_json::json!({"id":id,"path":path.to_string_lossy(),"bytes":bytes,"sha256":snapshot.sha256,
            "provenance":if snapshot.preserved_mime {"preservedMimeBody"} else {"reconstructed"},
            "originalMimeAvailable":snapshot.preserved_mime,"headersReconstructed":true}))
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn export_eml_data(&mut self, id: u64, token: Option<u64>) -> CmdResult {
        use std::io::Read;
        self.poll_source_exports();
        let (name, bytes, preserved_mime) = if let Some(token) = token {
            if self.source_exports.eml_size(id, token)? > native::DATA_MAX as u64 {
                return Err(crate::cmd::fail("this message needs chunked export; read its existing token with mail.exportEml"));
            }
            let mut snapshot = self.source_exports.take_eml(id, token)?;
            let mut bytes = Vec::new();
            snapshot.file.read_to_end(&mut bytes).map_err(|why| crate::cmd::fail(why.to_string()))?;
            (snapshot.name, bytes, snapshot.preserved_mime)
        } else {
            let input = native::Input::new(self, id, Kind::Eml)?;
            let mut output = native::Data::default();
            input.write_eml_to(&mut output).map_err(|why| crate::cmd::fail(why.to_string()))?;
            (crate::cmd::file::eml_name(&input.message), output.0, !input.message.draft && input.message.original.is_some())
        };
        Ok(serde_json::json!({"name":name,"bytes":bytes.len(),"data":crate::cmd::base64_encode(&bytes),
            "provenance":if preserved_mime {"preservedMimeBody"} else {"reconstructed"},
            "originalMimeAvailable":preserved_mime,"headersReconstructed":true}))
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::{self, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::thread::JoinHandle;

    use mailcraft_mime::ContentPart;
    use mailcraft_model::Message;
    use ring::rand::SecureRandom;
    use serde_json::{Value, json};

    use super::Kind;
    use crate::{
        CmdError, CmdResult, Session,
        cmd::{base64_encode, fail},
        io::ContentSource,
    };

    const MAX_SNAPSHOTS: usize = 4;
    const PAGE_MAX: usize = 64 << 10;
    pub(super) const DATA_MAX: usize = 4 << 20;

    #[derive(Default)]
    pub(super) struct Data(pub(super) Vec<u8>);
    impl Write for Data {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > DATA_MAX {
                return Err(io::Error::other("this message needs chunked export; use mail.exportEml instead of a whole data response"));
            }
            self.0.try_reserve(bytes.len()).map_err(|_| io::Error::other("memory for this export is unavailable"))?;
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    pub(super) struct Input {
        pub(super) message: Arc<Message>,
        path: Option<PathBuf>,
        source: Option<Arc<dyn ContentSource>>,
        kind: Kind,
        offset_minutes: i32,
    }

    impl Input {
        pub(super) fn new(session: &Session, id: u64, kind: Kind) -> Result<Self, CmdError> {
            let message = session.mailbox.messages.get(&id).cloned().ok_or_else(|| fail("the message is no longer in this mailbox"))?;
            let preserved_mime = kind == Kind::Eml && !message.draft && message.original.is_some();
            if !preserved_mime
                && (message.unfetched || message.incomplete || message.not_downloaded != 0 || message.kept.as_ref().is_some_and(|part| part.failed))
            {
                return Err(fail("the complete message is not available locally; download it before opening its source or exporting it"));
            }
            let offset_minutes = session.offset_at(message.date);
            Ok(Self { message, path: session.path.clone(), source: session.source(), kind, offset_minutes })
        }

        fn open(&self, part: ContentPart) -> io::Result<Box<dyn Read + '_>> {
            let message = &self.message;
            let kept = message.kept.as_deref().filter(|part| !part.failed);
            let (reference, bytes): (_, &[u8]) = match part {
                ContentPart::Text => (kept.and_then(|part| part.text).filter(|_| message.body.text.is_empty()), message.body.text.as_bytes()),
                ContentPart::Html => (
                    kept.and_then(|part| part.html).filter(|_| message.body.html.is_none()),
                    message.body.html.as_deref().unwrap_or_default().as_bytes(),
                ),
                ContentPart::Attachment(index) => {
                    let part = message.attachments.get(index).ok_or_else(|| io::Error::other("the attachment no longer exists"))?;
                    (part.kept.filter(|_| part.data.is_empty()), part.data.as_slice())
                }
                ContentPart::Original => {
                    let original = message.original.as_ref().ok_or_else(|| io::Error::other("the retained original is unavailable"))?;
                    (original.kept.filter(|_| original.data.is_empty()), original.data.as_slice())
                }
            };
            if let Some(reference) = reference {
                let (Some(source), Some(path)) = (&self.source, &self.path) else {
                    return Err(io::Error::other("the retained content has no readable source"));
                };
                source.open(path, &reference).map(|reader| reader as Box<dyn Read>).map_err(io::Error::other)
            } else {
                Ok(Box::new(Cursor::new(bytes)))
            }
        }

        pub(super) fn write_eml_to(&self, output: &mut dyn Write) -> io::Result<()> {
            let mut nonce = [0u8; 16];
            ring::rand::SystemRandom::new().fill(&mut nonce).map_err(|_| io::Error::other("the export boundary could not be generated"))?;
            let nonce = nonce.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            // Preserve Save Message As's established draft-only Bcc contract.
            let options = mailcraft_mime::WriteOptions { include_bcc: self.message.draft, offset_minutes: self.offset_minutes };
            mailcraft_mime::write_to(&self.message, &options, &nonce, &mut |part| self.open(part), output)
        }

        pub(super) fn produce(&self, cancelled: Arc<AtomicBool>) -> io::Result<Snapshot> {
            let mut file = tempfile::tempfile()?;
            let html = self.message.body.html.is_some() || self.message.kept.as_ref().is_some_and(|part| part.html.is_some());
            let format = if self.kind == Kind::Eml {
                "eml"
            } else if html {
                "html"
            } else {
                "text"
            };
            {
                let mut output = BufWriter::with_capacity(PAGE_MAX, Cancellable { file: &mut file, cancelled: cancelled.clone() });
                if self.kind == Kind::Eml {
                    self.write_eml_to(&mut output)?;
                } else {
                    let part = if html { ContentPart::Html } else { ContentPart::Text };
                    if html {
                        crate::source_format::pretty(self.open(part)?, &mut output)?;
                    } else {
                        io::copy(&mut self.open(part)?, &mut output)?;
                    }
                }
                output.flush()?;
            }
            file.sync_all()?;
            let len = file.seek(SeekFrom::End(0))?;
            file.seek(SeekFrom::Start(0))?;
            let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
            let mut bytes = [0u8; PAGE_MAX];
            loop {
                if cancelled.load(Ordering::Acquire) {
                    return Err(io::Error::other("source export cancelled"));
                }
                let count = file.read(&mut bytes)?;
                if count == 0 {
                    break;
                }
                hash.update(&bytes[..count]);
            }
            let sha256 = hash.finish().as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
            file.seek(SeekFrom::Start(0))?;
            Ok(Snapshot {
                file,
                len,
                sha256,
                format,
                name: crate::cmd::file::eml_name(&self.message),
                preserved_mime: !self.message.draft && self.message.original.is_some(),
            })
        }
    }

    struct Cancellable<'a> {
        file: &'a mut File,
        cancelled: Arc<AtomicBool>,
    }
    impl Write for Cancellable<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(io::Error::other("source export cancelled"));
            }
            self.file.write(bytes)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    pub(super) struct Snapshot {
        pub(super) file: File,
        pub(super) len: u64,
        pub(super) sha256: String,
        format: &'static str,
        pub(super) name: String,
        pub(super) preserved_mime: bool,
    }
    enum State {
        Pending { result: mpsc::Receiver<io::Result<Snapshot>>, worker: Option<JoinHandle<()>>, cancelled: Arc<AtomicBool> },
        Ready(Snapshot),
        Failed(String),
        Closed,
    }
    struct Entry {
        message: u64,
        kind: Kind,
        state: State,
    }

    #[derive(Default)]
    pub(crate) struct Exports {
        next: u64,
        opening: Option<u64>,
        entries: BTreeMap<u64, Entry>,
    }

    impl Exports {
        #[cfg(any(test, feature = "test-support"))]
        pub(super) fn wait(&mut self) {
            for entry in self.entries.values_mut() {
                if let State::Pending { worker, .. } = &mut entry.state
                    && let Some(worker) = worker.take()
                {
                    let _ = worker.join();
                }
            }
            self.poll();
        }

        pub(super) fn bind(&mut self, opening: u64) {
            if self.opening == Some(opening) {
                return;
            }
            // Mailbox-local IDs may be reused by another opening. Revoke the
            // old snapshots and join their workers before accepting a new read;
            // token numbers themselves are never reused in this session.
            self.cancel_and_join();
            self.entries.clear();
            self.opening = Some(opening);
        }

        fn cancel_and_join(&mut self) {
            for entry in self.entries.values_mut() {
                if let State::Pending { cancelled, worker, .. } = &mut entry.state {
                    cancelled.store(true, Ordering::Release);
                    if let Some(worker) = worker.take() {
                        let _ = worker.join();
                    }
                }
            }
        }
        pub(super) fn busy(&self) -> bool {
            self.entries.values().any(|entry| matches!(entry.state, State::Pending { .. }))
        }

        pub(super) fn prune(&mut self, mailbox: &mailcraft_model::Mailbox) {
            self.entries.retain(|_, entry| {
                if mailbox.message(entry.message).is_some() {
                    return true;
                }
                if let State::Pending { cancelled, .. } = &entry.state {
                    cancelled.store(true, Ordering::Release);
                    true
                } else {
                    false
                }
            });
        }

        pub(super) fn take_eml(&mut self, id: u64, token: u64) -> Result<Snapshot, CmdError> {
            let entry = self
                .entries
                .get(&token)
                .filter(|entry| entry.message == id && entry.kind == Kind::Eml)
                .ok_or_else(|| fail("the prepared message token is closed or belongs to another message"))?;
            match &entry.state {
                State::Ready(_) => (),
                State::Pending { .. } => return Err(fail("the message export is still being prepared")),
                State::Failed(why) => return Err(fail(why.as_str())),
                State::Closed => return Err(fail("the prepared message token is closed")),
            }
            match self.entries.remove(&token).map(|entry| entry.state) {
                Some(State::Ready(mut snapshot)) => {
                    snapshot.file.seek(SeekFrom::Start(0)).map_err(|why| fail(why.to_string()))?;
                    Ok(snapshot)
                }
                _ => Err(fail("the prepared message is no longer available")),
            }
        }

        pub(super) fn eml_size(&self, id: u64, token: u64) -> Result<u64, CmdError> {
            match self.entries.get(&token).filter(|entry| entry.message == id && entry.kind == Kind::Eml).map(|entry| &entry.state) {
                Some(State::Ready(snapshot)) => Ok(snapshot.len),
                _ => Err(fail("the complete prepared message is not ready or its token is closed")),
            }
        }

        pub(super) fn poll(&mut self) {
            for entry in self.entries.values_mut() {
                let State::Pending { result, worker, cancelled } = &mut entry.state else { continue };
                let result = match result.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => continue,
                    Err(mpsc::TryRecvError::Disconnected) => Err(io::Error::other("the source export stopped unexpectedly")),
                };
                let joined = worker.take().is_none_or(|worker| worker.join().is_ok());
                let was_cancelled = cancelled.load(Ordering::Acquire);
                entry.state = match result {
                    _ if was_cancelled && joined => State::Closed,
                    Ok(snapshot) if joined => State::Ready(snapshot),
                    Ok(_) => State::Failed("the source export stopped unexpectedly".into()),
                    Err(why) => State::Failed(why.to_string()),
                };
            }
        }

        pub(super) fn begin(&mut self, input: Input) -> CmdResult {
            if self.busy() {
                return Err(fail("another source export is being prepared; observe its completion first"));
            }
            // A cancelled pending export ends as Closed only to acknowledge its
            // owner's close; it never holds one of the bounded snapshot slots.
            self.entries.retain(|_, entry| !matches!(entry.state, State::Closed));
            if self.entries.len() >= MAX_SNAPSHOTS {
                return Err(fail("close a previous source/export token before opening another"));
            }
            let token = self.next.checked_add(1).ok_or_else(|| fail("source export identifiers are exhausted"))?;
            let (message, kind) = (input.message.id, input.kind);
            let cancelled = Arc::new(AtomicBool::new(false));
            let signal = cancelled.clone();
            let (sender, result) = mpsc::channel();
            let worker = std::thread::Builder::new()
                .name("mail source export".into())
                .spawn(move || {
                    let _ = sender.send(input.produce(signal));
                })
                .map_err(|_| fail("the native source export worker could not start"))?;
            self.next = token;
            self.entries.insert(token, Entry { message, kind, state: State::Pending { result, worker: Some(worker), cancelled } });
            Ok(json!({"id":message,"token":token,"state":"pending"}))
        }

        pub(super) fn reply(&mut self, id: u64, kind: Kind, token: u64, offset: u64, limit: usize, close: bool) -> CmdResult {
            if !(4..=PAGE_MAX).contains(&limit) {
                return Err(fail("source pages contain 4 to 65536 bytes"));
            }
            let entry = self
                .entries
                .get_mut(&token)
                .filter(|entry| entry.message == id && entry.kind == kind)
                .ok_or_else(|| fail("this source token is closed or belongs to another message"))?;
            if close {
                if let State::Pending { cancelled, .. } = &entry.state {
                    cancelled.store(true, Ordering::Release);
                    return Ok(json!({"id":id,"token":token,"state":"closing"}));
                }
                self.entries.remove(&token);
                return Ok(json!({"id":id,"token":token,"state":"closed"}));
            }
            match &mut entry.state {
                State::Closed => Ok(json!({"id":id,"token":token,"state":"closed"})),
                State::Pending { .. } => Ok(json!({"id":id,"token":token,"state":"pending"})),
                State::Failed(why) => Err(fail(why.as_str())),
                State::Ready(snapshot) => {
                    if offset > snapshot.len {
                        return Err(fail("the offset is past the end of this source"));
                    }
                    snapshot.file.seek(SeekFrom::Start(offset)).map_err(|why| fail(why.to_string()))?;
                    let mut bytes = Vec::new();
                    Read::by_ref(&mut snapshot.file).take(limit as u64).read_to_end(&mut bytes).map_err(|why| fail(why.to_string()))?;
                    if kind == Kind::Body {
                        match std::str::from_utf8(&bytes) {
                            Ok(_) => (),
                            Err(why) if why.error_len().is_none() && why.valid_up_to() > 0 => bytes.truncate(why.valid_up_to()),
                            Err(_) => {
                                return Err(fail(
                                    "the decoded source is not valid UTF-8 or the offset is inside a character; use its returned continuation offset",
                                ));
                            }
                        }
                    }
                    let end = offset.saturating_add(bytes.len() as u64);
                    let mut value = json!({"id":id,"token":token,"state":"ready","offset":offset,
                        "nextOffset":if end < snapshot.len {Some(end)} else {None},"bytes":snapshot.len,"sha256":snapshot.sha256,
                        "format":snapshot.format,"provenance":if kind == Kind::Eml {if snapshot.preserved_mime {"preservedMimeBody"} else {"reconstructed"}} else {"decodedBody"},"eof":end == snapshot.len});
                    if kind == Kind::Eml {
                        value["name"] = Value::String(snapshot.name.clone());
                        value["data"] = Value::String(base64_encode(&bytes));
                        value["originalMimeAvailable"] = Value::Bool(snapshot.preserved_mime);
                        value["headersReconstructed"] = Value::Bool(true);
                        if !snapshot.preserved_mime {
                            value["originalMimeUnavailableReason"] = Value::String(
                                "The retained message contains decoded bodies and attachments, not an original MIME body available for export."
                                    .into(),
                            );
                        }
                    } else {
                        let text = std::str::from_utf8(&bytes)
                            .map_err(|_| fail("the source page is not on a UTF-8 boundary; use its returned continuation offset"))?;
                        let (shown, truncated) = crate::cmd::mail::mail_source::visible(text, 1 << 20);
                        if truncated {
                            return Err(fail("the visible source page exceeds its representation bound"));
                        }
                        value["source"] = Value::String(shown);
                        value["formatted"] = Value::Bool(snapshot.format == "html");
                        value["representation"] = Value::String("visibleText".into());
                    }
                    Ok(value)
                }
            }
        }
    }

    impl Drop for Exports {
        fn drop(&mut self) {
            self.cancel_and_join();
        }
    }
}
