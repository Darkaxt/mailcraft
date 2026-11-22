// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Local archive kept packed while nobody looks at it (0.12.30).
//!
//! The archive's messages are most of a mailbox and are looked at once a month. While they
//! aren't, each is kept here as a packed copy of itself (made and read back by the engine,
//! `mailcraft_engine::parking`) instead of in [`crate::Mailbox::messages`]: what a folder holds is
//! still counted from these, and the mailbox file still has every one of them. Nothing here is
//! written anywhere: a message is parked only once the file has it as it is.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::{ContentRef, Id, KeptBody};

/// One parked message: what the counts need, and the message itself packed.
#[derive(Clone, PartialEq, Eq)]
pub struct ParkedMessage {
    pub account: Id,
    pub folder: Id,
    pub read: bool,
    /// The message as JSON, compressed.
    pub packed: Box<[u8]>,
    /// Where its content is kept, which the JSON doesn't say ([`crate::Message::kept`] and the
    /// rest of what is never written).
    pub kept: Kept,
}

/// What of a message is never written as JSON: where each piece of its content is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    pub body: Option<KeptBody>,
    pub attachments: Vec<Option<ContentRef>>,
    pub original: Option<ContentRef>,
    pub headers: Option<ContentRef>,
}

impl std::fmt::Debug for ParkedMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ParkedMessage(folder {}, {} bytes)", self.folder, self.packed.len())
    }
}

/// The parked messages of a mailbox, by id: none of them is in [`crate::Mailbox::messages`].
#[derive(Clone, Default)]
pub struct Parked {
    messages: Arc<BTreeMap<Id, Arc<ParkedMessage>>>,
    /// What each folder holds of them: messages, unread.
    folders: Arc<HashMap<Id, (usize, usize)>>,
}

impl PartialEq for Parked {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.messages, &other.messages) || self.messages == other.messages
    }
}

impl std::fmt::Debug for Parked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parked({} messages)", self.messages.len())
    }
}

impl Parked {
    /// These messages, parked.
    pub fn of(messages: BTreeMap<Id, Arc<ParkedMessage>>) -> Parked {
        let mut folders: HashMap<Id, (usize, usize)> = HashMap::new();
        for m in messages.values() {
            let f = folders.entry(m.folder).or_default();
            f.0 += 1;
            if !m.read {
                f.1 += 1;
            }
        }
        Parked { messages: Arc::new(messages), folders: Arc::new(folders) }
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn contains(&self, id: &Id) -> bool {
        self.messages.contains_key(id)
    }

    pub fn get(&self, id: &Id) -> Option<&Arc<ParkedMessage>> {
        self.messages.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Id, &Arc<ParkedMessage>)> {
        self.messages.iter()
    }

    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        self.messages.keys().copied()
    }

    /// The parked messages of `folder`: how many, and how many unread.
    pub fn in_folder(&self, folder: Id) -> (usize, usize) {
        self.folders.get(&folder).copied().unwrap_or_default()
    }

    /// The folders that have parked messages, with what they hold.
    pub fn folders(&self) -> impl Iterator<Item = (Id, (usize, usize))> + '_ {
        self.folders.iter().map(|(id, held)| (*id, *held))
    }

    /// The packed bytes, together.
    pub fn bytes(&self) -> usize {
        self.messages.values().map(|m| m.packed.len()).sum()
    }

    /// With `more` parked besides.
    pub fn with(&self, more: impl IntoIterator<Item = (Id, Arc<ParkedMessage>)>) -> Parked {
        let mut messages = (*self.messages).clone();
        messages.extend(more);
        Parked::of(messages)
    }

    /// Without those of `ids` (they are in the mailbox's messages again, or gone).
    pub fn without(&self, ids: impl IntoIterator<Item = Id>) -> Parked {
        let mut messages = (*self.messages).clone();
        let mut any = false;
        for id in ids {
            any |= messages.remove(&id).is_some();
        }
        if any { Parked::of(messages) } else { self.clone() }
    }

    /// Without any that `has` says is in the mailbox's messages (as an Undo puts one back).
    pub fn settled(&self, has: impl Fn(&Id) -> bool) -> Parked {
        let resident: Vec<Id> = self.messages.keys().filter(|id| has(id)).copied().collect();
        if resident.is_empty() { self.clone() } else { self.without(resident) }
    }
}
