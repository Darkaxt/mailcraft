// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes of an IMAP account's folders made here, made to the server's mailboxes
//! ([`SyncRequest::folder_changes`], v0.12.14): a folder renamed or moved is a RENAME of its
//! mailbox, which takes those under it along, and a folder deleted is a DELETE of its mailbox.
//!
//! Each change goes on its own: one the server refuses is said, and the others are made. None is
//! made to INBOX, to a mailbox the server keeps for a purpose of its own (`\Sent`, `\Trash`,
//! `\All`, …, RFC 6154), to one a standard folder here follows, nor to one holding any of these.
//! Nothing is renamed into the server's Trash (Gmail would keep it as a label under it): a folder
//! moved there is deleted instead. A mailbox is deleted only when it holds nothing beyond what
//! the owner had of it here (AU3-H7): mail that arrived since, or a mailbox under it, keeps it.

use std::collections::HashSet;

use mailcraft_i18n::said;
use mailcraft_model::FolderKind;
use mailcraft_model::sync::{
    ImapFolderChange, ImapFolderDone, ImapFolderOutcome, MAX_SHOWN_MESSAGES, MailboxRenamed, Shown, SyncRequest, SyncStage, SyncStep,
};
use mailcraft_proto::ProtoError;
use mailcraft_proto::imap::{Imap, Mailbox, decode_utf7, encode_utf7};

use crate::sync::{Run, because};
use crate::transport::Stream;
use crate::{NetError, kinds};

/// The special-use flags (RFC 6154) of the mailboxes the server keeps for a purpose of its own,
/// and INBOX's: never renamed or deleted from here.
const SPECIAL_USE: [&str; 9] = ["\\Inbox", "\\Sent", "\\Drafts", "\\Trash", "\\Junk", "\\All", "\\Archive", "\\Flagged", "\\Important"];

/// Most changes of folders made in one run; the rest wait for the next.
const MAX_CHANGES: usize = 500;

/// Why a change went no further: refused (by the server, or here), with a line saying why, or
/// the connection was lost.
enum Stop {
    Refused(String),
    Lost(NetError),
}

impl From<ProtoError> for Stop {
    fn from(e: ProtoError) -> Stop {
        let e = NetError::from(e);
        if e.is_fatal() { Stop::Lost(e) } else { Stop::Refused(e.said().to_string()) }
    }
}

/// Make the request's changes of folders, the renames first. What became of each is in the
/// run's answer (`folders_changed`), handed over before the folders are listed. Answers the
/// paths of the mailboxes renamed now, which aren't read this run (the folders here know them
/// by their old paths until the answer is taken in); `None` when the connection was lost, and
/// what was not done waits for the next run.
pub(crate) fn change_folders(run: &mut Run, imap: &mut Imap<Stream<'_>>) -> Option<HashSet<String>> {
    let request = run.request;
    let mut renamed_now = HashSet::new();
    let mut list = match imap.list() {
        Ok(list) => list,
        Err(e) => {
            let e = NetError::from(e);
            run.out.problem(because(said!("Couldn't change the folders on the server"), &e));
            return (!e.is_fatal()).then_some(renamed_now);
        }
    };
    let renames = request.folder_changes.iter().filter(|c| matches!(c, ImapFolderChange::Rename { .. }));
    let deletes = request.folder_changes.iter().filter(|c| matches!(c, ImapFolderChange::Delete { .. }));
    // Each mailbox's path before and after, of every rename of this run, in order: a later change
    // names a mailbox as the folders here knew it.
    let mut done: Vec<(String, String)> = Vec::new();
    let mut personal: Option<(String, Option<char>)> = None;
    for change in renames.chain(deletes).take(MAX_CHANGES) {
        let made = match change {
            ImapFolderChange::Rename { from, parent, name, .. } => rename(imap, &mut list, &mut done, &mut personal, request, (from, parent, name)),
            ImapFolderChange::Delete { remote, shown } => delete(imap, &mut list, &done, request, remote, shown),
        };
        let outcome = match made {
            Ok(outcome) => outcome,
            Err(Stop::Refused(why)) => ImapFolderOutcome::Refused { why },
            Err(Stop::Lost(e)) => {
                run.out.problem(because(said!("Couldn't change the folders on the server"), &e));
                run.hand_over_in(imap, SyncStep::new(SyncStage::Updating, "", 0, 0));
                return None;
            }
        };
        if let ImapFolderOutcome::Done { renamed } = &outcome {
            renamed_now.extend(renamed.iter().map(|r| r.to.clone()));
        }
        run.out.folders_changed.push(ImapFolderDone { change: change.clone(), outcome });
    }
    run.hand_over_in(imap, SyncStep::new(SyncStage::Updating, "", 0, 0));
    Some(renamed_now)
}

/// `path` as it is after the renames `done` of this run.
fn translated(path: &str, done: &[(String, String)]) -> String {
    let mut path = path.to_string();
    for (from, to) in done {
        if path == *from {
            path.clone_from(to);
        }
    }
    path
}

/// Rename the mailbox `from` to `name` under the mailbox `parent` (empty: the top level).
fn rename(
    imap: &mut Imap<Stream<'_>>,
    list: &mut [Mailbox],
    done: &mut Vec<(String, String)>,
    personal: &mut Option<(String, Option<char>)>,
    request: &SyncRequest,
    (from, parent, name): (&str, &str, &str),
) -> Result<ImapFolderOutcome, Stop> {
    let (from, parent) = (translated(from, done), translated(parent, done));
    let Some(source) = list.iter().find(|m| kinds::remote_name(m) == from) else {
        // Renamed by a run whose answer was lost, or gone from the server.
        let to = target(imap, list, personal, &parent, name, None)?;
        if !list.iter().any(|m| m.raw == to) {
            return Ok(ImapFolderOutcome::Gone);
        }
        let renamed: Vec<MailboxRenamed> = tree(list, &to)
            .map(|m| MailboxRenamed { from: format!("{from}{}", decode_utf7(&m.raw[to.len()..])), to: kinds::remote_name(m) })
            .collect();
        done.extend(renamed.iter().map(|r| (r.from.clone(), r.to.clone())));
        return Ok(ImapFolderOutcome::Done { renamed });
    };
    let (raw, own) = (source.raw.clone(), source.delimiter);
    if let Some(why) = untouchable(list, request, &raw) {
        return Err(Stop::Refused(why));
    }
    let to = target(imap, list, personal, &parent, name, own)?;
    if to == raw {
        return Ok(ImapFolderOutcome::Done { renamed: Vec::new() });
    }
    if in_trash(list, request, &to) {
        return Err(Stop::Refused(said!("a folder isn't kept in the server's Trash; delete it instead").to_string()));
    }
    if own.is_some_and(|d| to.starts_with(&format!("{raw}{d}"))) {
        return Err(Stop::Refused(said!("a folder can't move into itself").to_string()));
    }
    if list.iter().any(|m| m.raw == to) {
        return Err(Stop::Refused(said!("the server has a folder of that name there").to_string()));
    }
    imap.rename(&raw, &to)?;
    // Other mail programs list subscribed mailboxes only; one that won't is no reason to stop.
    if let Err(e) = imap.subscribe(&to) {
        let e = NetError::from(e);
        if e.is_fatal() {
            return Err(Stop::Lost(e));
        }
    }
    let below = own.map(|d| format!("{raw}{d}"));
    let mut renamed = Vec::new();
    for m in list.iter_mut() {
        if m.raw == raw || below.as_deref().is_some_and(|b| m.raw.starts_with(b)) {
            let was = kinds::remote_name(m);
            m.raw = format!("{to}{}", &m.raw[raw.len()..]);
            m.name = decode_utf7(&m.raw);
            renamed.push(MailboxRenamed { from: was, to: kinds::remote_name(m) });
        }
    }
    done.extend(renamed.iter().map(|r| (r.from.clone(), r.to.clone())));
    Ok(ImapFolderOutcome::Done { renamed })
}

/// Delete the mailbox `remote`, when it holds nothing beyond `shown` (AU3-H7).
fn delete(
    imap: &mut Imap<Stream<'_>>,
    list: &mut Vec<Mailbox>,
    done: &[(String, String)],
    request: &SyncRequest,
    remote: &str,
    shown: &Shown,
) -> Result<ImapFolderOutcome, Stop> {
    let remote = translated(remote, done);
    let Some(at) = list.iter().position(|m| kinds::remote_name(m) == remote) else {
        return Ok(ImapFolderOutcome::Gone);
    };
    let raw = list[at].raw.clone();
    if let Some(why) = untouchable(list, request, &raw) {
        return Err(Stop::Refused(why));
    }
    // A mailbox under it was made elsewhere, or wasn't deleted (those deleted here go first).
    let mut items = tree(list, &raw).filter(|m| m.raw != raw && !m.has_flag("\\NonExistent")).count();
    if kinds::selectable(&list[at]) {
        let selected = imap.select(&raw, true)?;
        if usize::try_from(selected.exists).unwrap_or(usize::MAX) > MAX_SHOWN_MESSAGES {
            return Err(Stop::Refused(said!("it holds more mail than can be checked").to_string()));
        }
        if selected.exists > 0 {
            let (_, seen) = shown.sets();
            let held = imap.uid_fetch_flags("1:*")?;
            items += held
                .iter()
                .filter(|m| !m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Deleted")))
                .filter(|m| !seen.contains(&Shown::key(&Shown::imap_message(selected.uid_validity, m.uid))))
                .count();
        }
        // Not deleted while it is open: the Inbox is opened in its place, only to be looked at.
        if let Err(e) = imap.select("INBOX", true) {
            let e = NetError::from(e);
            if e.is_fatal() {
                return Err(Stop::Lost(e));
            }
        }
    }
    if items > 0 {
        return Ok(ImapFolderOutcome::NotShown { items: u32::try_from(items).unwrap_or(u32::MAX) });
    }
    imap.delete(&raw)?;
    list.remove(at);
    Ok(ImapFolderOutcome::Done { renamed: Vec::new() })
}

/// The mailbox `raw` and those under it, by the server's names.
fn tree<'a>(list: &'a [Mailbox], raw: &'a str) -> impl Iterator<Item = &'a Mailbox> {
    let own = list.iter().find(|m| m.raw == raw).and_then(|m| m.delimiter);
    list.iter().filter(move |m| m.raw == raw || own.is_some_and(|d| m.raw.strip_prefix(raw).is_some_and(|rest| rest.starts_with(d))))
}

/// Why the mailbox `raw` is not to be renamed or deleted from here, when it isn't: it, or one
/// under it, is INBOX, one the server keeps for a purpose of its own, or one a standard folder
/// here follows.
fn untouchable(list: &[Mailbox], request: &SyncRequest, raw: &str) -> Option<String> {
    let standard = |m: &Mailbox| {
        let remote = kinds::remote_name(m);
        kinds::is_inbox(m)
            || SPECIAL_USE.iter().any(|flag| m.has_flag(flag))
            || request.folders.iter().any(|f| f.kind != FolderKind::Custom && f.remote == remote)
    };
    tree(list, raw).any(standard).then(|| said!("it is one of the server's own folders, or holds one").to_string())
}

/// Whether the name `to` is under the server's Trash.
fn in_trash(list: &[Mailbox], request: &SyncRequest, to: &str) -> bool {
    let trash = |m: &Mailbox| {
        let remote = kinds::remote_name(m);
        m.has_flag("\\Trash") || request.folders.iter().any(|f| f.kind == FolderKind::Deleted && f.remote == remote)
    };
    list.iter().any(|m| trash(m) && m.delimiter.is_some_and(|d| to.strip_prefix(m.raw.as_str()).is_some_and(|rest| rest.starts_with(d))))
}

/// The name, as the server wants it, of a mailbox `name` under the mailbox `parent` (empty: at
/// the top level of the personal namespace, RFC 2342), as [`crate::push`] makes a mailbox.
/// `own`: the separator of the mailbox renamed, for one at the top level.
fn target(
    imap: &mut Imap<Stream<'_>>,
    list: &[Mailbox],
    personal: &mut Option<(String, Option<char>)>,
    parent: &str,
    name: &str,
    own: Option<char>,
) -> Result<String, Stop> {
    let (mut raw, delimiter) = if parent.is_empty() {
        if personal.is_none() {
            *personal = Some(personal_namespace(imap)?);
        }
        let (prefix, delimiter) = personal.clone().unwrap_or_default();
        let inbox = list.iter().find(|m| kinds::is_inbox(m)).and_then(|m| m.delimiter);
        (prefix, delimiter.or(own).or(inbox).or_else(|| list.iter().find_map(|m| m.delimiter)).unwrap_or('/'))
    } else {
        let Some(above) = list.iter().find(|m| kinds::remote_name(m) == parent) else {
            return Err(Stop::Refused(said!("the folder it goes into is not on the server").to_string()));
        };
        let Some(delimiter) = above.delimiter else {
            return Err(Stop::Refused(said!("the server keeps no folders inside others").to_string()));
        };
        (above.raw.clone(), delimiter)
    };
    // A name can't hold the character that parts the levels.
    let level: String = name.chars().filter(|c| !c.is_control()).map(|c| if c == delimiter { '_' } else { c }).collect();
    let level = level.trim();
    if level.is_empty() {
        return Err(Stop::Refused(said!("the folder has no usable name").to_string()));
    }
    if !raw.is_empty() && !raw.ends_with(delimiter) {
        raw.push(delimiter);
    }
    raw.push_str(&encode_utf7(level));
    Ok(raw)
}

/// The personal namespace's prefix and separator (RFC 2342); none from a server that doesn't say.
fn personal_namespace(imap: &mut Imap<Stream<'_>>) -> Result<(String, Option<char>), Stop> {
    let capabilities = match imap.capabilities() {
        Ok(capabilities) => capabilities,
        Err(ProtoError::Rejected(_)) => return Ok((String::new(), None)),
        Err(e) => return Err(e.into()),
    };
    if !capabilities.iter().any(|c| c == "NAMESPACE") {
        return Ok((String::new(), None));
    }
    match imap.namespace() {
        Ok(Some(ns)) => Ok((ns.prefix, ns.delimiter)),
        Ok(None) | Err(ProtoError::Rejected(_)) => Ok((String::new(), None)),
        Err(e) => Err(e.into()),
    }
}
