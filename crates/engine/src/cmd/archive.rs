// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The local archive (v0.12.10): a Microsoft account's mail older than a number of years
//! ([`Account::archive_after`]) is kept on this computer only, so that the account's storage on
//! the server is freed. Each folder's old mail goes into its **twin**, a folder of the same name
//! under "On this computer" ([`Folder::twin_of`]), and the list of a folder shows its twin's mail
//! with its own: the folders stay as they were. The server's copy goes once the message is whole
//! here, saved, and in a backup ([`Session::note_backed_up`]), and goes for good (it isn't kept
//! recoverable on the server, which would still count against the account's storage).

use std::collections::{BTreeMap, HashSet};

use mailcraft_model::{AccountKind, Folder, FolderKind, Id, Mailbox, Message};
use mailcraft_time::Date;
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdResult, CommandSpec, Session, p};

/// Messages moved by one step ([`step`]).
pub const MOST_AT_ONCE: usize = 5000;
/// No more is moved while this many of an account's still wait to leave its server: the moves
/// go at the pace their mail is fetched and backed up.
pub const MOST_WAITING: usize = 100_000;
/// The most years an account can keep in the cloud before the rest is archived.
pub const MOST_YEARS: u64 = 50;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("archive.preview", "Preview Local Archive", "File › Options › Archive", preview).reads().params(
            r#"{"account": number, "years": number} — read-only: how many messages of the Microsoft account `account` are older than `years` years, and their size: what archive.set would keep on this computer only (`count`, `bytes`). Drafts, the Outbox, Deleted Items, Junk Email and Snoozed are left out"#,
        ),
        CommandSpec::new("archive.set", "Keep Old Mail on This Computer", "File › Options › Archive", set).params(
            r#"{"account": number, "years": number | null} — a Microsoft account's mail older than `years` years (1 to 50) is moved, a batch at a time, into the same folders under "On this computer", and removed from the server for good once it is whole here, saved and backed up (backups must be on). The folders' lists show it as before. `null` stops archiving more (mail archived stays here)"#,
        ),
        CommandSpec::new("archive.status", "Local Archive Status", "File › Options › Archive", status)
            .reads()
            .params(r#"{} — read-only: for each Microsoft account, `years` (null when off), `toMove` (still to be archived), `waiting` (archived here, still on the server) and `waitingBackup` (of those, whole but not backed up yet)"#),
        CommandSpec::new("archive.step", "Archive Old Mail", "Send / Receive", run_step).pure().params(
            r#"{} — moves the next batch of old mail of every account with archiving on into its folders on this computer (at most 5,000 at a time, and none while 100,000 still wait to leave a server). Answers `moved`"#,
        ),
    ]
}

/// The time `years` years before `now`: mail received before it is archived.
pub fn cutoff(now: i64, years: u32) -> i64 {
    let day = Date::from_days(now.div_euclid(86_400));
    let year = day.year - years as i32;
    let then = Date::new(year, day.month, day.day).or_else(|| Date::new(year, day.month, 28)).unwrap_or(day);
    then.to_days() * 86_400
}

/// When a message came: received, else its own date.
fn when(m: &Message) -> i64 {
    if m.received > 0 { m.received } else { m.date }
}

/// The folders of `account` whose mail stays where it is: Drafts, the Outbox, Deleted Items,
/// Junk Email, Snoozed and the folders under them.
fn kept_in_place(mb: &Mailbox, account: Id) -> HashSet<Id> {
    let mut kept: HashSet<Id> = mb
        .folders
        .iter()
        .filter(|f| f.account == account && matches!(f.kind, FolderKind::Drafts | FolderKind::Outbox | FolderKind::Deleted | FolderKind::Junk))
        .flat_map(|f| mb.subtree(f.id))
        .collect();
    if let Some(snoozed) = mb.snoozed_folder(account) {
        kept.extend(mb.subtree(snoozed));
    }
    kept
}

/// Whether `account` is one whose old mail can be archived: a Microsoft account read through
/// Graph, not a shared mailbox.
fn archivable(mb: &Mailbox, account: Id) -> bool {
    mb.account(account).is_some_and(|a| a.kind == AccountKind::Microsoft && !a.is_shared()) && mb.is_graph(account)
}

/// The messages of `account` older than `years` years at `now` that are to be archived, oldest
/// first: on the server, not being moved out already, not a draft, and with all their content
/// (one the server gave only in part stays).
pub fn candidates(mb: &Mailbox, account: Id, years: u32, now: i64) -> Vec<Id> {
    let before = cutoff(now, years);
    let kept = kept_in_place(mb, account);
    let mut old: Vec<(i64, Id)> = mb
        .messages
        .values()
        .filter(|m| m.account == account && !kept.contains(&m.folder))
        .filter(|m| (1..before).contains(&when(m)))
        .filter(|m| m.moving_out.is_none() && !m.draft && !m.incomplete)
        .filter(|m| m.server.as_ref().is_some_and(|s| !s.gone && !s.id.is_empty()))
        .map(|m| (when(m), m.id))
        .collect();
    old.sort_unstable();
    old.into_iter().map(|(_, id)| id).collect()
}

/// The size of the messages `ids` as their server gave it (what the account's storage counts).
fn size_of(mb: &Mailbox, ids: &[Id]) -> u64 {
    ids.iter().filter_map(|id| mb.message(*id)).map(|m| m.server.as_ref().map_or(0, |s| s.size).max(m.size())).sum()
}

/// The archive's top folder for `account` under "On this computer" (made when missing): the
/// twin of the account itself ([`Folder::twin_of`]).
fn top(mb: &mut Mailbox, account: Id) -> Result<Id, String> {
    let local = mb.local_account();
    if let Some(f) = mb.folders.iter().find(|f| f.account == local && f.twin_of == Some(account)) {
        return Ok(f.id);
    }
    let email = mb.account(account).map(|a| if a.email.is_empty() { a.name.clone() } else { a.email.clone() }).unwrap_or_default();
    let name = format!("{email} archive");
    let id = make(mb, local, None, &name)?;
    mb.folder_mut(id).map_err(|e| e.to_string())?.twin_of = Some(account);
    Ok(id)
}

/// A folder named `name` under `parent` of `account`: the one there with no twin of its own, else
/// one made (with a number after the name when the name is taken).
fn make(mb: &mut Mailbox, account: Id, parent: Option<Id>, name: &str) -> Result<Id, String> {
    // (A name on this computer has no slash.)
    let name = name.replace('/', "-");
    let name = name.as_str();
    let free =
        mb.folders.iter().find(|f| f.account == account && f.parent == parent && f.twin_of.is_none() && f.name.eq_ignore_ascii_case(name.trim()));
    if let Some(f) = free {
        return Ok(f.id);
    }
    let mut last = String::new();
    for n in 1..=20 {
        let named = if n == 1 { name.to_string() } else { format!("{name} ({n})") };
        match mb.add_folder(account, parent, &named) {
            Ok(id) => return Ok(id),
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

/// The twin of the folder `folder` (made, and the twins of the folders it lies in, when missing).
pub fn twin(mb: &mut Mailbox, folder: Id) -> Result<Id, String> {
    let f: Folder = mb.folder(folder).cloned().ok_or_else(|| format!("folder {folder} not found"))?;
    if let Some(t) = mb.folders.iter().find(|t| t.twin_of == Some(folder)) {
        return Ok(t.id);
    }
    let parent = match f.parent {
        Some(p) => twin(mb, p)?,
        None => top(mb, f.account)?,
    };
    let local = mb.local_account();
    let id = make(mb, local, Some(parent), &f.name)?;
    mb.folder_mut(id).map_err(|e| e.to_string())?.twin_of = Some(folder);
    Ok(id)
}

/// The twins of the folders `folders`, to show with them.
pub fn twins_of(mb: &Mailbox, folders: &HashSet<Id>) -> Vec<Id> {
    mb.folders.iter().filter(|f| f.twin_of.is_some_and(|of| folders.contains(&of))).map(|f| f.id).collect()
}

/// Whether `m` was archived and still waits to leave its server.
pub fn waiting(m: &Message) -> bool {
    m.moving_out.as_ref().is_some_and(|out| out.archive)
}

/// Whether `m` is whole here: every part of it fetched and readable ([`waiting`] mail goes into a
/// backup only so).
pub fn whole(m: &Message) -> bool {
    !m.unfetched && !m.incomplete && m.not_downloaded == 0 && !m.kept.as_deref().is_some_and(|k| k.failed)
}

/// Archive the next batch of old mail of every account with archiving on. How many moved.
pub fn step(mb: &mut Mailbox, now: i64) -> usize {
    let on: Vec<(Id, u32)> = mb.accounts.iter().filter(|a| a.archive_after > 0).map(|a| (a.id, a.archive_after)).collect();
    let mut moved = 0;
    for (account, years) in on {
        if !archivable(mb, account) {
            continue;
        }
        let still = mb.messages.values().filter(|m| waiting(m) && m.moving_out.as_ref().is_some_and(|o| o.account == account)).count();
        let room = MOST_WAITING.saturating_sub(still).min(MOST_AT_ONCE.saturating_sub(moved));
        if room == 0 {
            continue;
        }
        let mut by_folder: BTreeMap<Id, Vec<Id>> = BTreeMap::new();
        for id in candidates(mb, account, years, now).into_iter().take(room) {
            if let Some(m) = mb.message(id) {
                by_folder.entry(m.folder).or_default().push(id);
            }
        }
        for (folder, ids) in by_folder {
            // A folder whose twin can't be made, or whose mail can't move, waits: the rest goes on.
            let done = match twin(mb, folder).and_then(|to| mb.move_messages(&ids, to).map_err(|e| e.to_string())) {
                Ok(done) => done,
                Err(e) => {
                    log::warn!("archive: {e}");
                    continue;
                }
            };
            for (id, _) in done {
                if let Ok(m) = mb.message_mut(id)
                    && let Some(out) = m.moving_out.as_mut()
                {
                    out.archive = true;
                    moved += 1;
                }
            }
        }
    }
    moved
}

fn account_param(s: &Session, v: &Value) -> Result<Id, crate::CmdError> {
    let account = p::req_id(v, "account")?;
    if !archivable(&s.mailbox, account) {
        return Err(bad("choose a Microsoft account (not a shared mailbox)"));
    }
    Ok(account)
}

fn years_param(v: &Value) -> Result<Option<u32>, crate::CmdError> {
    match v.get("years") {
        None => Err(bad("`years` (a number of years, or null) is required")),
        Some(Value::Null) => Ok(None),
        Some(_) => match p::u64(v, "years") {
            Some(n @ 1..=MOST_YEARS) => Ok(Some(n as u32)),
            _ => Err(bad(format!("`years` must be a whole number from 1 to {MOST_YEARS}, or null"))),
        },
    }
}

fn preview(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["account", "years"])?;
    let account = account_param(s, v)?;
    let years = years_param(v)?.ok_or_else(|| bad("`years` must be a number"))?;
    let ids = candidates(&s.mailbox, account, years, s.now());
    Ok(json!({"count": ids.len(), "bytes": size_of(&s.mailbox, &ids), "before": cutoff(s.now(), years)}))
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["account", "years"])?;
    let account = account_param(s, v)?;
    let years = years_param(v)?;
    let a = s.mailbox.accounts.iter_mut().find(|a| a.id == account).ok_or_else(|| fail("account not found"))?;
    a.archive_after = years.unwrap_or(0);
    Ok(json!({"account": account, "years": years}))
}

fn status(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &[])?;
    let mb = &s.mailbox;
    let now = s.now();
    let rows: Vec<Value> = mb
        .accounts
        .iter()
        .filter(|a| archivable(mb, a.id))
        .map(|a| {
            let out: Vec<&Message> =
                mb.messages.values().map(|m| m.as_ref()).filter(|m| waiting(m) && m.moving_out.as_ref().is_some_and(|o| o.account == a.id)).collect();
            let unbacked = out.iter().filter(|m| whole(m) && !s.backed_up.contains(&m.id)).count();
            let to_move = if a.archive_after > 0 { candidates(mb, a.id, a.archive_after, now).len() } else { 0 };
            json!({
                "account": a.id,
                "email": a.email,
                "years": (a.archive_after > 0).then_some(a.archive_after),
                "toMove": to_move,
                "waiting": out.len(),
                "waitingBackup": unbacked,
            })
        })
        .collect();
    Ok(json!({"accounts": rows}))
}

fn run_step(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &[])?;
    let now = s.now();
    let moved = step(&mut s.mailbox, now);
    if moved > 0 {
        s.mailbox_changed();
    }
    Ok(json!({"moved": moved}))
}

/// Whether there is archived mail, whole here, that waits for a backup before its server's copy
/// can go.
pub fn waits_for_backup(s: &Session) -> bool {
    s.mailbox.messages.values().any(|m| waiting(m) && whole(m) && !s.backed_up.contains(&m.id))
}
