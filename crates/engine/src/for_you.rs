// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! fork: For you (docs/superpowers/specs/2026-11-26-for-you-design.md). A section at the top of
//! the list of All mail and of an Inbox (the unified one, or one account's) with the unread mail
//! worth reading first, in tiers: from a new sender, from people the owner replies to, from
//! senders whose links he opens, from senders he reads. The Inbox's also brings back the unread
//! mail rules filed into other folders: the folders he switched on always, the others when they
//! qualify by a tier.
//!
//! Worked out from the mailbox as it is whenever the list is built: nothing is stored but the
//! settings ([`ForYouPrefs`]) and the time a link of a message was opened here
//! ([`mailcraft_model::Message::link_opened`]). It never changes a message: reading one takes it
//! out of the section the next time the list is built.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::{Arc, PoisonError};

use mailcraft_model::sender_warnings::{self, Facts, FactsBuilder};
use mailcraft_model::{FolderKind, Id, Mailbox, Message};
use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::list::{Row, when};
use crate::{Filter, Module, Scope, Session, SortBy};

const DAY: Timestamp = 86_400;

/// The heading of the section, as the list's other headings are (in English; the window shows
/// it through `tl!`).
pub const HEADING: &str = "For you";
/// Which of its message's rows a row of the section is ([`Row::copy`]): the section is a list of
/// its own above the list (v0.12.45), and its messages are listed there too, as copy 0.
pub const COPY: u16 = u16::MAX;

/// Why a message is in the section. In the order the section lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    /// In a folder whose "Show in the Inbox's For you" is on (the Inbox's section alone).
    Folder,
    /// From a sender first seen within [`ForYouPrefs::new_sender_days`].
    NewSender,
    /// From a sender the owner answered at least once among the last [`ForYouPrefs::replied_last`].
    Replied,
    /// From a sender in more than half of whose mail of [`ForYouPrefs::links_days`] a link was opened.
    Links,
    /// From a sender more than half of whose mail of [`ForYouPrefs::read_days`] is read.
    Read,
}

impl Tier {
    /// What the row's tip says of why it is here (English; the window shows it through `tl!`).
    pub fn reason(self) -> &'static str {
        match self {
            Tier::Folder => "Its folder is shown in the Inbox's For you",
            Tier::NewSender => "A new sender",
            Tier::Replied => "Someone you reply to",
            Tier::Links => "A sender whose links you open",
            Tier::Read => "A sender you read",
        }
    }
}

/// For you's settings (File › Options, the section's gear; `forYou.settings`): this computer's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ForYouPrefs {
    /// The section in All mail.
    pub all_mail: bool,
    /// The section in an Inbox (the unified one and each account's).
    pub inbox: bool,
    pub new_sender: bool,
    /// A sender is new for this many days after their first message here.
    pub new_sender_days: u16,
    pub replied: bool,
    /// Of how many of a sender's latest messages one answered keeps them in "People you reply to".
    pub replied_last: u16,
    pub links: bool,
    pub links_days: u16,
    pub read: bool,
    pub read_days: u16,
    /// The most rows of the tiers the section shows until Show all (those of switched-on folders
    /// are never held back). 0 is none.
    pub cap: u16,
    /// The names services give the owner as the one who did something (a GitHub notification of
    /// his own push is from "Darkaxt"): mail with one of them as its sender's name is his own
    /// doing and never in the section. Empty: the words of his own addresses ([`Own::of`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub own_names: Vec<String>,
    /// The folders with a setting of their own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<ForYouFolder>,
}

impl Default for ForYouPrefs {
    fn default() -> Self {
        // Off until turned on (`forYou.settings`), so that upstream's lists stay as they are.
        ForYouPrefs {
            all_mail: false,
            inbox: false,
            new_sender: true,
            new_sender_days: 30,
            replied: true,
            replied_last: 10,
            links: true,
            links_days: 61,
            read: true,
            read_days: 61,
            cap: 15,
            own_names: Vec::new(),
            folders: Vec::new(),
        }
    }
}

impl ForYouPrefs {
    /// The most days a window may be, the most messages "People you reply to" looks back over,
    /// the largest cap, and the longest age limit (a year).
    pub const MAX_DAYS: u16 = 3_650;
    pub const MAX_LAST: u16 = 1_000;
    pub const MAX_CAP: u16 = 500;
    pub const MAX_AGE_MINUTES: u32 = 525_600;

    /// The setting of folder `id`, when it has one.
    pub fn folder(&self, id: Id) -> Option<&ForYouFolder> {
        self.folders.iter().find(|f| f.folder == id)
    }

    /// Every number within its bounds and the folders once each, as a settings file may say anything.
    pub fn tidy(&mut self) {
        for days in [&mut self.new_sender_days, &mut self.links_days, &mut self.read_days] {
            *days = (*days).clamp(1, Self::MAX_DAYS);
        }
        self.replied_last = self.replied_last.clamp(1, Self::MAX_LAST);
        self.cap = self.cap.min(Self::MAX_CAP);
        self.own_names = self.own_names.iter().map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).take(50).collect();
        self.own_names.dedup();
        let mut seen = HashSet::new();
        self.folders.retain(|f| seen.insert(f.folder) && (f.always || f.max_age_minutes.is_some()));
        for f in &mut self.folders {
            f.max_age_minutes = f.max_age_minutes.map(|m| m.clamp(1, Self::MAX_AGE_MINUTES));
        }
    }
}

/// A folder's own For you setting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ForYouFolder {
    pub folder: Id,
    /// "Show in the Inbox's For you": its unread mail is always there.
    pub always: bool,
    /// Its mail leaves the section once older than this (an OTP's 30 minutes). `None`: when read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_minutes: Option<u32>,
}

/// What one sender's mail says, as [`SenderStats::of`] counts it.
#[derive(Clone, Debug, Default)]
struct Stat {
    /// When their first message here arrived.
    first: Timestamp,
    /// Their latest messages' arrival and whether each was answered, at most `replied_last`.
    latest: Vec<(Timestamp, bool)>,
    links_total: u32,
    links_opened: u32,
    read_total: u32,
    read_read: u32,
}

/// Each sender's mail, by address in lower case, over every account and folder but drafts.
#[derive(Clone, Debug, Default)]
pub struct SenderStats {
    by: HashMap<String, Stat>,
}

/// The key a sender's mail is counted under: the address in lower case.
fn sender_key(m: &Message) -> Option<String> {
    let email = m.from.email.trim();
    (!email.is_empty()).then(|| email.to_lowercase())
}

impl SenderStats {
    /// Counted over `messages` as of `now`, with the windows of `p`.
    pub fn of<'a>(messages: impl IntoIterator<Item = &'a Message>, now: Timestamp, p: &ForYouPrefs) -> SenderStats {
        let last = usize::from(p.replied_last.max(1));
        let links_from = now.saturating_sub(Timestamp::from(p.links_days) * DAY);
        let read_from = now.saturating_sub(Timestamp::from(p.read_days) * DAY);
        let mut by: HashMap<String, Stat> = HashMap::new();
        for m in messages {
            if m.draft {
                continue;
            }
            let Some(key) = sender_key(m) else { continue };
            let at = when(m);
            let st = by.entry(key).or_insert_with(|| Stat { first: at, ..Stat::default() });
            st.first = st.first.min(at);
            if st.latest.len() < last {
                st.latest.push((at, m.answered));
            } else if let Some(oldest) = st.latest.iter_mut().min_by_key(|(t, _)| *t)
                && oldest.0 < at
            {
                *oldest = (at, m.answered);
            }
            if at >= links_from {
                st.links_total += 1;
                st.links_opened += u32::from(m.link_opened.is_some());
            }
            if at >= read_from {
                st.read_total += 1;
                st.read_read += u32::from(m.read);
            }
        }
        SenderStats { by }
    }

    fn get(&self, m: &Message) -> Option<&Stat> {
        self.by.get(&sender_key(m)?)
    }
}

/// The owner's own addresses and names, for telling mail of his own doing.
#[derive(Clone, Debug, Default)]
pub struct Own {
    /// His accounts' addresses and sender addresses, in lower case.
    pub addresses: HashSet<String>,
    /// The names services give him, in lower case ([`ForYouPrefs::own_names`]).
    pub names: Vec<String>,
}

impl Own {
    pub fn of(mb: &Mailbox, p: &ForYouPrefs) -> Own {
        let addresses: HashSet<String> = mb
            .accounts
            .iter()
            .flat_map(|a| std::iter::once(a.email.as_str()).chain(a.sender_addresses().map(|x| x.email.as_str())))
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        let names = if p.own_names.is_empty() { names_of(&addresses) } else { p.own_names.iter().map(|n| n.trim().to_lowercase()).collect() };
        Own { addresses, names }
    }
}

/// The words of the local parts of `addresses` (`darkaxt.remaxku@…` gives darkaxt and remaxku):
/// those of four letters or more, once each.
fn names_of(addresses: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = addresses
        .iter()
        .filter_map(|a| a.split('@').next())
        .flat_map(|local| local.split(['.', '_', '-', '+']))
        .filter(|w| w.chars().count() >= 4)
        .map(str::to_string)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Whether `m` is mailing-list or machine mail: it has `List-Unsubscribe`, `Auto-Submitted`
/// other than `no`, `Precedence` bulk, list or junk, a no-reply address or a `[bot]` name.
pub fn is_bulk(m: &Message) -> bool {
    let auto = m.header("Auto-Submitted").is_some_and(|v| !v.trim().eq_ignore_ascii_case("no"));
    let precedence = m.header("Precedence").is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "bulk" | "list" | "junk"));
    let local = m.from.email.split('@').next().unwrap_or_default().to_ascii_lowercase();
    let no_reply = ["noreply", "no-reply", "no_reply", "donotreply", "do-not-reply", "do_not_reply"].iter().any(|p| local.starts_with(p));
    let bot = m.from.name.trim().to_ascii_lowercase().ends_with("[bot]");
    m.header("List-Unsubscribe").is_some() || auto || precedence || no_reply || bot
}

/// Whether `m` is of the owner's own doing: from one of his addresses, or with one of his names
/// as its sender's name.
pub fn self_triggered(m: &Message, own: &Own) -> bool {
    let email = m.from.email.trim().to_lowercase();
    let name = m.from.name.trim().to_lowercase();
    own.addresses.contains(&email) || (!name.is_empty() && own.names.contains(&name))
}

/// The highest tier of the tiers of `p` the unread message `m` is in, by its sender's mail
/// (`stats`) as of `now`; `None` for a message that is read, a draft, of the owner's own doing,
/// or in none. (Sender warnings and folders are the section's to judge: [`section`].)
pub fn tier(m: &Message, stats: &SenderStats, p: &ForYouPrefs, own: &Own, now: Timestamp) -> Option<Tier> {
    if m.read || m.draft || self_triggered(m, own) {
        return None;
    }
    let st = stats.get(m)?;
    let new_from = now.saturating_sub(Timestamp::from(p.new_sender_days) * DAY);
    if p.new_sender && !is_bulk(m) && st.first >= new_from {
        return Some(Tier::NewSender);
    }
    if p.replied && st.latest.iter().any(|(_, answered)| *answered) {
        return Some(Tier::Replied);
    }
    if p.links && st.links_opened.saturating_mul(2) > st.links_total {
        return Some(Tier::Links);
    }
    if p.read && st.read_read.saturating_mul(2) > st.read_total {
        return Some(Tier::Read);
    }
    None
}

/// How strongly the sender of `m` qualifies for tier `t`, in thousandths: the share of their
/// latest messages answered, of their mail with a link opened, of their mail read. Within a tier
/// the section lists the strongest first, the newest first among equals; a new sender's and a
/// switched-on folder's mail go by date alone (0).
pub fn strength(t: Tier, m: &Message, stats: &SenderStats) -> u16 {
    let Some(st) = stats.get(m) else { return 0 };
    let share = |part: u64, whole: u64| (part.min(whole) * 1000).checked_div(whole).map_or(0, |v| u16::try_from(v).unwrap_or(1000));
    match t {
        Tier::Folder | Tier::NewSender => 0,
        Tier::Replied => share(st.latest.iter().filter(|(_, answered)| *answered).count() as u64, st.latest.len() as u64),
        Tier::Links => share(u64::from(st.links_opened), u64::from(st.links_total)),
        Tier::Read => share(u64::from(st.read_read), u64::from(st.read_total)),
    }
}

/// What of For you the message list is kept under ([`Session::rows`]): its settings, and while a
/// folder's mail leaves the Inbox's section by age, the minute.
pub(crate) fn list_key(s: &Session) -> String {
    let mut key = serde_json::to_string(&s.prefs.for_you).unwrap_or_default();
    if s.prefs.for_you.folders.iter().any(|f| f.max_age_minutes.is_some()) && applies(s) == Some(Where::Inbox) {
        key.push_str(&format!("@{}", s.now().div_euclid(60)));
    }
    key
}

/// Which list has the section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    AllMail,
    /// The accounts whose Inbox is in view.
    Inbox,
}

/// Whether the list in view has the section: All mail or an Inbox, by date, with no search and
/// every message or only the unread ones shown, and the section on for it.
pub fn applies(s: &Session) -> Option<Where> {
    let p = &s.prefs.for_you;
    let v = &s.view;
    let plain = v.module == Module::Mail
        && v.sort == SortBy::Date
        && matches!(v.filter, Filter::All | Filter::Unread)
        && mailcraft_model::search::Query::parse(&v.search).is_empty()
        && v.search_folder.is_none()
        && v.filing.is_none()
        && v.gmail.is_none();
    if !plain {
        return None;
    }
    if v.scope == Scope::Inboxes {
        return match (v.kind, v.all_mail) {
            (None, true) => p.all_mail.then_some(Where::AllMail),
            (None, false) => p.inbox.then_some(Where::Inbox),
            _ => None,
        };
    }
    let inbox = match v.unified {
        Some(kind) => kind == FolderKind::Inbox,
        None => v.scope == Scope::Folder && s.mailbox.folder(v.folder).is_some_and(|f| f.kind == FolderKind::Inbox),
    };
    (inbox && p.inbox).then_some(Where::Inbox)
}

/// The accounts whose Inbox is in view.
fn inbox_accounts(s: &Session) -> HashSet<Id> {
    let folders = if s.view.scope == Scope::Inboxes {
        s.mailbox.folders.iter().filter(|f| f.kind == FolderKind::Inbox).map(|f| f.id).collect()
    } else {
        crate::unified::in_view(s).unwrap_or_else(|| vec![s.view.folder])
    };
    folders.iter().filter_map(|f| s.mailbox.folder(*f)).map(|f| f.account).collect()
}

/// Whether mail in folder `id` was filed there: a folder mail arrives in or is filed to that
/// is not an Inbox, nor the Archive or under it, nor the local archive's.
pub fn filed_folder(s: &Session, id: Id) -> bool {
    let Some(f) = s.mailbox.folder(id) else { return false };
    if f.kind == FolderKind::Inbox || f.twin_of.is_some() || !crate::cmd::filing::receiving_folder(s, id) {
        return false;
    }
    let mut next = Some(id);
    for _ in 0..s.mailbox.folders.len() {
        let Some(folder) = next.and_then(|id| s.mailbox.folder(id)) else { break };
        if folder.kind == FolderKind::Archive {
            return false;
        }
        next = folder.parent;
    }
    true
}

/// Whether `m` is past its folder's age limit.
fn too_old(p: &ForYouPrefs, m: &Message, now: Timestamp) -> bool {
    p.folder(m.folder).and_then(|f| f.max_age_minutes).is_some_and(|minutes| when(m) < now.saturating_sub(Timestamp::from(minutes) * 60))
}

/// What the sender statistics were counted for: the mailbox as it is, the hour (the windows
/// move with the clock) and the windows.
type StatsKey = (u64, u64, u64, usize, Timestamp, String);

/// The kept statistics ([`Session::for_you_stats`]).
pub(crate) type StatsCache = std::sync::Mutex<Option<(StatsKey, Arc<SenderStats>)>>;

impl Session {
    /// The sender statistics of the mailbox as it is, counted once for it (they are the same for
    /// every list built until it changes).
    pub fn for_you_stats(&self) -> Arc<SenderStats> {
        let p = &self.prefs.for_you;
        let now = self.now();
        let windows = format!("{}|{}|{}|{}", p.new_sender_days, p.replied_last, p.links_days, p.read_days);
        let key: StatsKey = (self.openings(), self.restored(), self.rev(), self.mailbox.messages.len(), now / 3_600, windows);
        let mut cache = self.for_you_stats.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, stats)) = cache.as_ref()
            && *at == key
        {
            return stats.clone();
        }
        let stats = Arc::new(SenderStats::of(self.mailbox.messages.values().map(|m| m.as_ref()), now, p));
        *cache = Some((key, stats.clone()));
        stats
    }

    /// Settles what the section measures sender warnings against before the list is built
    /// ([`Session::rows`]): only when the list in view has the section.
    pub(crate) fn for_you_ready(&mut self) {
        if applies(self).is_some() {
            let _ = self.known_facts();
        }
    }

    /// Notes that a link of message `id` was opened from where it is read (the time, never the
    /// link). Not a step to undo, and nothing goes to a server.
    pub fn note_link_opened(&mut self, id: Id) {
        let now = self.now();
        let Ok(m) = self.mailbox.message_mut(id) else { return };
        m.link_opened = Some(now);
        self.dirty = true;
        self.rev += 1;
    }

    /// Forgets every link opened (`forYou.clearLinks`); how many messages had one.
    pub fn clear_link_history(&mut self) -> usize {
        let ids: Vec<Id> = self.mailbox.messages.values().filter(|m| m.link_opened.is_some()).map(|m| m.id).collect();
        for id in &ids {
            if let Ok(m) = self.mailbox.message_mut(*id) {
                m.link_opened = None;
            }
        }
        if !ids.is_empty() {
            self.dirty = true;
            self.rev += 1;
        }
        ids.len()
    }
}

/// One row the section may list: the tiers of its messages that qualify, best first, with the
/// row's arrival.
struct Candidate {
    at: Timestamp,
    row: Row,
    /// (tier, message), best first; the first not warned is the row's.
    tiers: Vec<(Tier, Id)>,
    next: usize,
}

/// Puts the rows of the section at the top of `out` (the list's rows, sorted by date, without
/// headings yet), the first wearing [`HEADING`], each as row [`COPY`] of its message: the window
/// shows them as a list of their own above the list, which still lists every message (v0.12.45).
/// By tier, then [`strength`], then newest first. How many there are; nothing when the list in
/// view has no section.
pub(crate) fn section(s: &Session, out: &mut Vec<Row>) -> usize {
    let Some(place) = applies(s) else { return 0 };
    let p = &s.prefs.for_you;
    let mb = &s.mailbox;
    let now = s.now();
    let stats = s.for_you_stats();
    let own = Own::of(mb, p);
    let kept = s.kept();
    let kept = |id: &Id| kept.as_ref().is_some_and(|k| k.contains(id));
    // A selected message keeps the place it had in the section though it was read since (as a
    // selected message stays in a list of unread mail, #428).
    let before: HashMap<Id, Tier> = s
        .rows_last()
        .map(|rows| rows.iter().filter_map(|r| r.for_you.map(|t| (r, t))).flat_map(|(r, t)| r.members.iter().map(move |id| (*id, t))).collect())
        .unwrap_or_default();
    let folder_tier = |m: &Message| {
        (place == Where::Inbox && p.folder(m.folder).is_some_and(|f| f.always) && filed_folder(s, m.folder) && !m.read).then_some(Tier::Folder)
    };
    let tier_of = |m: &Message| -> Option<Tier> {
        if m.pinned || too_old(p, m, now) {
            return None;
        }
        // (Kept from the list before, which may have been the Inbox's: a folder's place is the
        // Inbox's only.)
        let kept_tier = before.get(&m.id).copied().filter(|t| kept(&m.id) && (place == Where::Inbox || *t != Tier::Folder));
        folder_tier(m).or_else(|| tier(m, &stats, p, &own, now)).or(kept_tier)
    };
    let tiers_of = |members: &[Id]| -> Vec<(Tier, Id)> {
        let mut t: Vec<(Tier, Id)> = members.iter().filter_map(|id| mb.message(*id)).filter_map(|m| tier_of(m).map(|t| (t, m.id))).collect();
        t.sort();
        t
    };
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut listed: HashSet<Id> = HashSet::new();
    for row in out.iter() {
        listed.extend(row.members.iter().chain(&row.others).copied());
        // An open conversation or run, and what is listed under it, aren't the section's.
        if row.under.is_some() || row.open || mb.message(row.id).is_none_or(|m| m.pinned) {
            continue;
        }
        let tiers = tiers_of(&row.members);
        if !tiers.is_empty() {
            let at = mb.message(row.id).map(when).unwrap_or_default();
            candidates.push(Candidate { at, row: row.clone(), tiers, next: 0 });
        }
    }
    if place == Where::Inbox {
        extras(s, &listed, &tier_of, &mut candidates);
    }
    if candidates.is_empty() {
        return 0;
    }
    // Sender warnings are looked at only for the rows that would be shown: a message that has
    // them is never in the section, and its row goes by its next message that qualifies.
    let facts = s.last_facts().unwrap_or_else(|| Arc::new(FactsBuilder::default().accounts(mb.accounts.iter()).build()));
    let warned = |id: Id| mb.message(id).is_none_or(|m| warned(s, m, &facts));
    let strong = |t: Tier, id: Id| mb.message(id).map_or(0, |m| strength(t, m, &stats));
    let mut heap: BinaryHeap<(Reverse<Tier>, u16, Timestamp, Reverse<usize>)> =
        candidates.iter().enumerate().filter_map(|(i, c)| c.tiers.first().map(|(t, id)| (Reverse(*t), strong(*t, *id), c.at, Reverse(i)))).collect();
    let cap = if s.view.for_you_all || p.cap == 0 { usize::MAX } else { usize::from(p.cap) };
    let mut chosen: Vec<(usize, Tier)> = Vec::new();
    let mut capped = 0usize;
    let mut more = false;
    while let Some((Reverse(t), _, at, Reverse(i))) = heap.pop() {
        let Some(c) = candidates.get_mut(i) else { continue };
        let Some(&(_, id)) = c.tiers.get(c.next) else { continue };
        if warned(id) {
            c.next += 1;
            if let Some((next, id)) = c.tiers.get(c.next) {
                heap.push((Reverse(*next), strong(*next, *id), at, Reverse(i)));
            }
            continue;
        }
        if t != Tier::Folder {
            if capped >= cap {
                more = true;
                break;
            }
            capped += 1;
        }
        chosen.push((i, t));
    }
    if chosen.is_empty() {
        return 0;
    }
    let mut rows: Vec<Row> =
        chosen.iter().filter_map(|(i, t)| candidates.get(*i).map(|c| Row { for_you: Some(*t), group: None, copy: COPY, ..c.row.clone() })).collect();
    if let Some(first) = rows.first_mut() {
        first.group = Some(HEADING.into());
        first.for_you_more = more;
    }
    let n = rows.len();
    rows.append(out);
    *out = rows;
    n
}

/// The rows of the unread mail filed into folders of the accounts whose Inbox is in view that
/// isn't `listed` already and qualifies (`tier_of`): one per conversation in conversation view.
fn extras(s: &Session, listed: &HashSet<Id>, tier_of: &dyn Fn(&Message) -> Option<Tier>, out: &mut Vec<Candidate>) {
    let mb = &s.mailbox;
    let accounts = inbox_accounts(s);
    let filed: HashSet<Id> = mb.folders.iter().filter(|f| accounts.contains(&f.account) && filed_folder(s, f.id)).map(|f| f.id).collect();
    let kept = s.kept();
    let counts = s.fresh_counts();
    let messages: Vec<&Message> = match counts {
        Some(counts) => filed.iter().flat_map(|f| mb.messages.get_sorted(counts.ids(*f))).map(|m| m.as_ref()).collect(),
        None => mb.messages.values().map(|m| m.as_ref()).filter(|m| filed.contains(&m.folder)).collect(),
    };
    let threads = s.view.conversations.then(|| s.threads());
    let mut index: HashMap<(Id, mailcraft_model::ThreadKey), usize> = HashMap::new();
    let start = out.len();
    for m in messages {
        let selected = kept.as_ref().is_some_and(|k| k.contains(&m.id));
        if !filed.contains(&m.folder) || listed.contains(&m.id) || (m.read && !selected) || !crate::list::tag_kept(s, m) {
            continue;
        }
        let Some(t) = tier_of(m) else { continue };
        let at = when(m);
        let thread = threads.as_ref().map(|th| (crate::list::conversation_account(s, m), th.key(m.id)));
        if let Some(c) = thread.and_then(|k| index.get(&k)).and_then(|i| out.get_mut(*i)) {
            c.row.members.push(m.id);
            c.row.unread += usize::from(!m.read);
            c.tiers.push((t, m.id));
            c.tiers.sort();
            if at > c.at {
                (c.at, c.row.id) = (at, m.id);
            }
            continue;
        }
        if let Some(k) = thread {
            index.insert(k, out.len());
        }
        out.push(Candidate { at, row: Row::of(m, 0), tiers: vec![(t, m.id)], next: 0 });
    }
    let newest = |id: &Id| Reverse(mb.message(*id).map(|m| (when(m), m.id)));
    for c in out.iter_mut().skip(start) {
        c.row.members.sort_by_cached_key(newest);
    }
}

/// Whether the sender warnings of `m` (its header alone) would show a banner.
fn warned(s: &Session, m: &Message, facts: &Facts) -> bool {
    let account = s.mailbox.account(m.account);
    let kind = s.mailbox.folder(m.folder).map(|f| f.kind);
    let w = sender_warnings::check(m, account, kind, facts, None::<std::iter::Empty<(&str, &str)>>);
    sender_warnings::banner_level(&w).is_some()
}
