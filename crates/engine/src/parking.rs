// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Local archive put away while nobody looks at it (0.12.30).
//!
//! The archive (the folders of the Local account that hold a server folder's old mail,
//! [`mailcraft_model::Folder::twin_of`]) is most of a mailbox and is looked at once a month. While
//! it isn't, each of its messages is kept packed ([`mailcraft_model::Parked`]: the message as
//! JSON, compressed) instead of in the mailbox's messages: the folders still count it, and the
//! mailbox file keeps every one of them as it is (a message is put away only as the file has it,
//! and a save leaves it there, `store`).
//!
//! It is taken out, all of it at once and from memory, when something wants it: the list scrolled
//! to its end in a folder that has archived mail, a search, a command that names one of its
//! messages or acts on a whole folder or mailbox, any change an agent asks for. That happens
//! before the command takes its copy of the mailbox, so that no step of the history ever sees it
//! come or go. It is put away again after [`UNUSED`] without use.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Weak};

use mailcraft_model::{FlagStatus, Id, Kept, Message, Parked, ParkedMessage};
use serde_json::Value;

use crate::Session;

/// How long the archive stays out unused before it is put away again, in seconds.
pub const UNUSED: f64 = 15.0 * 60.0;
/// How often the window asks whether it is time, in seconds.
const TEND_EVERY: f64 = 60.0;

/// What the session knows of its archive's comings and goings. Starts over with each mailbox.
#[derive(Default)]
pub(crate) struct State {
    /// The packed copies of the messages last taken out, while each is still the message it was
    /// (the same allocation): put away again, it needs no packing.
    back: BTreeMap<Id, (Arc<ParkedMessage>, Weak<Message>)>,
    /// Something used the archive since the window last asked ([`Session::touch_archive`]).
    touched: bool,
    /// When it was last used, by the window's clock (seconds).
    used: Option<f64>,
    /// When the window last asked whether to put it away.
    tended: Option<f64>,
    /// Put away, and still known to the mailbox file's record by where they were in memory
    /// ([`crate::store::unlink`] couldn't be done yet).
    unlink: BTreeSet<Id>,
}

/// Message `m`, packed: `None` when packing it doesn't give it back exactly as it is (it then
/// stays as it is).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn freeze(m: &Message) -> Option<ParkedMessage> {
    use std::io::Write;
    let json = serde_json::to_vec(m).ok()?;
    let mut z = flate2::write::DeflateEncoder::new(Vec::with_capacity(json.len() / 3), flate2::Compression::fast());
    z.write_all(&json).ok()?;
    let packed = z.finish().ok()?.into_boxed_slice();
    let kept = Kept {
        body: m.kept.as_deref().copied(),
        attachments: m.attachments.iter().map(|a| a.kept).collect(),
        original: m.original.as_ref().and_then(|o| o.kept),
        headers: m.header_block.kept,
    };
    let p = ParkedMessage { account: m.account, folder: m.folder, read: m.read, packed, kept };
    // Only what comes back as it went: nothing of a message is ever lost to packing.
    (thaw(&p).as_ref() == Some(m)).then_some(p)
}

/// The message `p` holds.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn thaw(p: &ParkedMessage) -> Option<Message> {
    use std::io::Read;
    let mut json = Vec::with_capacity(p.packed.len().saturating_mul(4));
    flate2::read::DeflateDecoder::new(&p.packed[..]).read_to_end(&mut json).ok()?;
    let mut m: Message = serde_json::from_slice(&json).ok()?;
    m.kept = p.kept.body.map(Box::new);
    if m.attachments.len() != p.kept.attachments.len() {
        return None;
    }
    for (a, k) in m.attachments.iter_mut().zip(&p.kept.attachments) {
        a.kept = *k;
    }
    if let Some(o) = m.original.as_mut() {
        o.kept = p.kept.original;
    }
    m.header_block.kept = p.kept.headers;
    Some(m)
}

/// Every message of `parked`, taken out for a while (a mailbox file written whole, a backup).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn thaw_all(parked: &Parked) -> Result<Vec<(Id, Arc<Message>)>, String> {
    let list: Vec<(Id, &Arc<ParkedMessage>)> = parked.iter().map(|(id, p)| (*id, p)).collect();
    let thawed = crate::store::fan_out(list.len(), |i| thaw(list[i].1));
    list.iter()
        .zip(thawed)
        .map(|((id, _), m)| m.flatten().map(|m| (*id, Arc::new(m))).ok_or_else(|| format!("message {id} of the local archive couldn't be read back")))
        .collect()
}

/// The archive's folders: those of the Local account that hold a server folder's old mail.
fn archive_folders(s: &Session) -> HashSet<Id> {
    s.mailbox.folders.iter().filter(|f| f.twin_of.is_some()).map(|f| f.id).collect()
}

/// Whether `m` may be put away: nothing waits on it, nothing lists it outside its folder.
fn restful(m: &Message) -> bool {
    !m.draft
        && !m.pinned
        && m.flag != FlagStatus::Flagged
        && m.flag_due.is_none()
        && m.follow_up.is_none()
        && m.snooze.is_none()
        && m.moving_out.is_none()
        && m.send_at.is_none()
        && !m.sending
        && m.held.is_empty()
        && m.answers.is_none()
        && m.rule_processing.is_none()
        && m.server_draft.is_none()
        && !m.refetch
        && !m.original_wanted
        && !m.original_asked
        && crate::cmd::archive::whole(m)
}

/// The commands that look at the whole mailbox, a whole folder or a whole account: the archive
/// is taken out for them first.
const WHOLE: &[&str] = &[
    "file.saveAs",
    "file.exportFolder",
    "folder.delete",
    "folder.empty",
    "account.remove",
    "account.thinOffline",
    "account.trimDevice",
    "mail.markAllRead",
    "mail.findFromSender",
    "mail.findRelated",
    "folder.newSearchFolder",
    "searchFolders.preview",
];

/// The ids `params` names, under the names commands give them.
fn named_ids(params: &Value) -> Vec<Id> {
    let mut out = Vec::new();
    for key in ["id", "ids", "message", "messages"] {
        match params.get(key) {
            Some(Value::Number(n)) => out.extend(n.as_u64()),
            Some(Value::Array(a)) => out.extend(a.iter().filter_map(Value::as_u64)),
            _ => {}
        }
    }
    out
}

impl Session {
    /// How many messages of the archive are put away.
    pub fn archive_parked(&self) -> usize {
        self.mailbox.parked.len()
    }

    /// How many messages put away the folders the list looks in hold: the list ends with a row
    /// that takes them out as it comes into view.
    pub fn archive_parked_in_view(&self) -> usize {
        let parked = &self.mailbox.parked;
        if parked.is_empty() {
            return 0;
        }
        let (everywhere, folders) = crate::list::view_folders(self);
        if everywhere {
            parked.folders().filter(|(f, _)| !folders.contains(f)).map(|(_, (n, _))| n).sum()
        } else {
            folders.iter().map(|f| parked.in_folder(*f).0).sum()
        }
    }

    /// Whether `id` is a message of the archive (out or put away).
    pub fn in_archive(&self, id: Id) -> bool {
        self.mailbox.parked.contains(&id) || self.mailbox.message(id).and_then(|m| self.mailbox.folder(m.folder)).is_some_and(|f| f.twin_of.is_some())
    }

    /// The archive was looked at (a row of it in view): it stays out another [`UNUSED`].
    pub fn touch_archive(&mut self) {
        self.archive.touched = true;
    }

    /// Whether command `id` with `params` wants the archive out first.
    pub(crate) fn archive_wanted(&self, id: &str, params: &Value, mutates: bool) -> bool {
        let parked = &self.mailbox.parked;
        if parked.is_empty() {
            return false;
        }
        if mutates && self.agent.is_some() {
            return true;
        }
        if WHOLE.contains(&id) {
            return true;
        }
        let searched = params.get("query").or_else(|| params.get("text")).and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty());
        if searched && matches!(id, "view.search" | "mail.list") {
            return true;
        }
        if named_ids(params).iter().any(|i| parked.contains(i)) {
            return true;
        }
        params.get("folder").and_then(Value::as_u64).is_some_and(|f| parked.in_folder(f).0 > 0)
    }

    /// Take the archive out: every message put away is in the mailbox's messages again. How many.
    pub fn load_archive(&mut self) -> usize {
        if self.mailbox.parked.is_empty() {
            return 0;
        }
        #[cfg(target_arch = "wasm32")]
        return 0;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let parked = std::mem::take(&mut self.mailbox.parked);
            let list: Vec<(Id, Arc<ParkedMessage>)> = parked.iter().map(|(id, p)| (*id, p.clone())).collect();
            let thawed = crate::store::fan_out(list.len(), |i| thaw(&list[i].1));
            let (mut stay, mut out) = (BTreeMap::new(), Vec::with_capacity(list.len()));
            for ((id, p), m) in list.into_iter().zip(thawed) {
                match m.flatten() {
                    Some(m) => {
                        let m = Arc::new(m);
                        self.archive.back.insert(id, (p, Arc::downgrade(&m)));
                        self.mailbox.messages.insert(id, m.clone());
                        out.push((id, m));
                    }
                    None => {
                        // (Never thrown away: it stays put away, and the file keeps it.)
                        log::error!("message {id} of the local archive couldn't be read back; it stays put away");
                        stay.insert(id, p);
                    }
                }
            }
            self.mailbox.parked = Parked::of(stay);
            self.archive.unlink.clear();
            if let Some(path) = &self.path {
                crate::store::relink(path, &out);
            }
            self.archive.touched = true;
            self.rev += 1;
            log::info!("the local archive is out: {} messages", out.len());
            out.len()
        }
    }

    /// Put the archive away when it hasn't been used for [`UNUSED`]: the window asks at every
    /// frame (`now` its clock, in seconds), and it looks at most every [`TEND_EVERY`]. A mailbox
    /// just opened puts it away at once. How many were put away.
    pub fn tend_archive(&mut self, now: f64) -> usize {
        if self.in_command() {
            return 0;
        }
        let Some(_path) = self.path.clone() else { return 0 };
        #[cfg(not(target_arch = "wasm32"))]
        if !self.archive.unlink.is_empty() && crate::store::unlink(&_path, &self.archive.unlink) {
            self.archive.unlink.clear();
        }
        let searching = !self.view.search.trim().is_empty();
        let reading = self.sel.messages.iter().any(|id| self.in_archive(*id));
        let st = &mut self.archive;
        if std::mem::take(&mut st.touched) || searching || reading {
            st.used = Some(now);
        }
        if st.tended.is_some_and(|t| now - t < TEND_EVERY) {
            return 0;
        }
        st.tended = Some(now);
        if st.used.is_some_and(|u| now - u < UNUSED) {
            return 0;
        }
        self.park_archive()
    }

    /// Put away every message of the archive that may be ([`restful`], not selected, not being
    /// read) and that the mailbox file has as it is. How many.
    pub(crate) fn park_archive(&mut self) -> usize {
        #[cfg(target_arch = "wasm32")]
        return 0;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = self.path.clone() else { return 0 };
            let Some(local) = self.mailbox.accounts.iter().find(|a| a.kind == mailcraft_model::AccountKind::Local).map(|a| a.id) else {
                return 0;
            };
            let archive = archive_folders(self);
            if archive.is_empty() {
                return 0;
            }
            let selected: HashSet<Id> = self.sel.messages.iter().copied().collect();
            let resident: HashSet<Id> = self.resident.iter().map(|(id, _)| *id).collect();
            let candidates: Vec<(Id, &Arc<Message>)> = self
                .mailbox
                .messages
                .iter()
                .filter(|(id, m)| m.account == local && archive.contains(&m.folder) && !selected.contains(id) && !resident.contains(id) && restful(m))
                .map(|(id, m)| (*id, m))
                .collect();
            if candidates.is_empty() {
                return 0;
            }
            let Some(saved) = crate::store::saved_as(&path, &candidates) else { return 0 };
            // Packed again only when it changed since it was taken out.
            let work: Vec<(Id, Arc<Message>, Option<Arc<ParkedMessage>>)> = candidates
                .into_iter()
                .zip(saved)
                .filter(|(_, saved)| *saved)
                .map(|((id, m), _)| {
                    let again = self.archive.back.get(&id).filter(|(_, was)| std::ptr::eq(was.as_ptr(), Arc::as_ptr(m))).map(|(p, _)| p.clone());
                    (id, m.clone(), again)
                })
                .collect();
            let packed = crate::store::fan_out(work.len(), |i| {
                let (_, m, again) = &work[i];
                again.clone().or_else(|| freeze(m).map(Arc::new))
            });
            let before = self.mailbox.messages.clone();
            let mut away = Vec::with_capacity(work.len());
            for ((id, _, _), p) in work.iter().zip(packed) {
                if let Some(p) = p.flatten() {
                    self.mailbox.messages.remove(id);
                    away.push((*id, p));
                }
            }
            if away.is_empty() {
                return 0;
            }
            let gone: BTreeSet<Id> = away.iter().map(|(id, _)| *id).collect();
            self.mailbox.parked = self.mailbox.parked.with(away);
            self.archive.back.clear();
            // The file's record lets go of them too: else each would keep the memory it took.
            self.archive.unlink.extend(gone.iter().copied());
            if crate::store::unlink(&path, &self.archive.unlink) {
                self.archive.unlink.clear();
            }
            // The steps of the history let go of them too (those that have them as they were put
            // away, and didn't change them), sharing what is left with the mailbox.
            let after = &self.mailbox.messages;
            for step in self.history.iter_mut().chain(self.redo.iter_mut()) {
                let touched = &step.touched;
                step.mailbox.messages.without_as(&before, after, &gone, |id| touched.changed_message(id));
            }
            drop(before);
            self.rev += 1;
            log::info!("the local archive is put away: {} messages, {} KB packed", gone.len(), self.mailbox.parked.bytes() / 1024);
            gone.len()
        }
    }

    /// After an Undo or Redo put back `back` in place of `now`: what is put away is what `now`
    /// had put away, but for what the step put back in the messages or took away
    /// ([`Session::swap`]).
    pub(crate) fn settle_parked(&mut self, now: &Parked, touched: &crate::undo::Touched) {
        let messages = &self.mailbox.messages;
        self.mailbox.parked = now.settled(|id| messages.contains_key(id) || touched.changed_message(id));
    }

    /// Before command `id`: the archive out, when it wants it ([`Session::archive_wanted`]).
    pub(crate) fn archive_for(&mut self, id: &str, params: &Value, mutates: bool) {
        if !self.in_command() && self.archive_wanted(id, params, mutates) {
            self.load_archive();
        }
    }

    /// A new mailbox: what was known of the old one's archive goes.
    pub(crate) fn forget_archive(&mut self) {
        self.archive = State::default();
    }
}
