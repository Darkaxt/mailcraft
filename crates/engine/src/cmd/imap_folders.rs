// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The changes of an IMAP account's folders made here, for its server (v0.12.14): a folder renamed
//! or moved ([`Queued::MoveFolder`]) has its mailbox renamed, and a folder deleted
//! ([`Queued::DeleteMailbox`]) has its mailbox deleted, as a Microsoft account's are
//! ([`super::graph_mail`]). What the server did is taken in before the folders it lists, so that a
//! mailbox renamed or waiting to be deleted is never taken for a new folder.
//!
//! A change waits while mail of the run, or mail still to leave, goes into or out of a mailbox it
//! is about: the moves out of a deleted folder's mailbox go first, and a mailbox is deleted only
//! once they are made.

use std::collections::HashSet;

use mailcraft_model::sync::{FolderState, ImapFolderChange, ImapFolderDone, ImapFolderOutcome, ImapPlace, MailboxRenamed, RemoteOp};
use mailcraft_model::{FolderKind, Id, Mailbox, Pending, Queued};

/// The changes of `account`'s folders its server is to make now, the order they were made in.
/// `ops` and `places`: the changes of mail this run makes, whose mailboxes are left alone.
pub(super) fn changes(mb: &Mailbox, account: Id, ops: &[RemoteOp], places: &[ImapPlace]) -> Vec<ImapFolderChange> {
    let mut busy: HashSet<&str> = ops
        .iter()
        .flat_map(|op| match op {
            RemoteOp::Move { from, to, .. } => [from.as_str(), to.as_str()],
            op => [op.mailbox(), op.mailbox()],
        })
        .chain(places.iter().map(|place| place.to.as_str()))
        .collect();
    // Mail still to leave its mailbox: in this run, or a later one.
    busy.extend(mb.pending.iter().filter(|p| p.account() == account && p.source().1 != 0).map(|p| p.source().0));
    let mut out = Vec::new();
    for queued in &mb.queued {
        match queued {
            Queued::MoveFolder { account: a, folder } if *a == account => {
                let Some(f) = mb.folder(*folder).filter(|f| f.account == account && f.kind == FolderKind::Custom && !f.remote.is_empty()) else {
                    continue;
                };
                // Under a folder whose mailbox the server doesn't have yet: it waits for it.
                let parent = match f.parent.and_then(|p| mb.folder(p)) {
                    None => String::new(),
                    Some(p) if p.remote.is_empty() => continue,
                    Some(p) => p.remote.clone(),
                };
                // (Nor while mail leaves a mailbox under it that no folder here follows any more: the
                // server renames that along. A name that only begins the same waits too.)
                let tree = mb.subtree(f.id);
                let moving = mb.folders.iter().filter(|g| tree.contains(&g.id) && !g.remote.is_empty()).any(|g| busy.contains(g.remote.as_str()))
                    || busy.iter().any(|b| b.starts_with(f.remote.as_str()));
                if moving || busy.contains(parent.as_str()) {
                    continue;
                }
                out.push(ImapFolderChange::Rename { folder: f.id, from: f.remote.clone(), parent, name: f.name.clone() });
            }
            Queued::DeleteMailbox { account: a, remote, shown, .. } if *a == account && !busy.contains(remote.as_str()) => {
                out.push(ImapFolderChange::Delete { remote: remote.clone(), shown: shown.clone() });
            }
            _ => {}
        }
    }
    out
}

/// The mailboxes of `account` waiting to be deleted that no folder here follows any more, as
/// folders for the run: the moves out of them are made under the `UIDVALIDITY` they had. (What a
/// run reads of them is not taken in: [`waits_to_go`].)
pub(super) fn leaving(mb: &Mailbox, account: Id) -> Vec<FolderState> {
    let followed: HashSet<&str> = mb.folders.iter().filter(|f| f.account == account && !f.remote.is_empty()).map(|f| f.remote.as_str()).collect();
    mb.queued
        .iter()
        .filter_map(|q| match q {
            Queued::DeleteMailbox { account: a, remote, uid_validity, .. }
                if *a == account && *uid_validity != 0 && !followed.contains(remote.as_str()) =>
            {
                Some(FolderState { remote: remote.clone(), kind: FolderKind::Custom, uid_validity: *uid_validity, ..Default::default() })
            }
            _ => None,
        })
        .collect()
}

/// Whether the mailbox `remote` of `account` waits to be deleted: what the server lists or says
/// of it is not taken for a folder.
pub(super) fn waits_to_go(mb: &Mailbox, account: Id, remote: &str) -> bool {
    mb.queued.iter().any(|q| matches!(q, Queued::DeleteMailbox { account: a, remote: r, .. } if *a == account && r == remote))
}

/// Take in what became of the changes of `account`'s folders; what to tell the owner goes into
/// `errors`.
pub(super) fn taken_in(mb: &mut Mailbox, account: Id, done: &[ImapFolderDone], lang: mailcraft_i18n::Lang, errors: &mut Vec<String>) {
    // Every path renamed so far, for the changes after.
    let mut renamed_all: Vec<(String, String)> = Vec::new();
    for d in done {
        match (&d.change, &d.outcome) {
            (ImapFolderChange::Rename { folder, parent, name, .. }, ImapFolderOutcome::Done { renamed }) => {
                for r in renamed {
                    follow_rename(mb, account, r);
                    renamed_all.push((r.from.clone(), r.to.clone()));
                }
                // Done, unless the folder was renamed or moved again meanwhile: that goes next.
                let parent = translated(parent, &renamed_all);
                let now =
                    mb.folder(*folder).map(|f| (f.name.clone(), f.parent.and_then(|p| mb.folder(p)).map(|p| p.remote.clone()).unwrap_or_default()));
                if now.is_none_or(|(n, p)| n == *name && p == parent) {
                    drop_move(mb, account, *folder);
                }
            }
            (ImapFolderChange::Rename { folder, name, .. }, ImapFolderOutcome::Refused { why }) => {
                // The folder stays as it is here; the server's stays as it is there.
                drop_move(mb, account, *folder);
                let name = mb.folder(*folder).map_or_else(|| shown(name), |f| shown(&f.name));
                errors.push(format!("Couldn't rename or move the folder \"{name}\" on the server: {why}"));
            }
            (ImapFolderChange::Rename { folder, .. }, ImapFolderOutcome::Gone | ImapFolderOutcome::NotShown { .. }) => {
                drop_move(mb, account, *folder)
            }
            (ImapFolderChange::Delete { remote, .. }, ImapFolderOutcome::Done { .. } | ImapFolderOutcome::Gone) => {
                drop_delete(mb, account, &translated(remote, &renamed_all));
            }
            (ImapFolderChange::Delete { remote, shown: what }, ImapFolderOutcome::NotShown { items }) => {
                // The server holds there what the owner hadn't seen (AU3-H7): it wasn't deleted, and
                // the listing brings the folder back here with it. Deleting it again asks again.
                drop_delete(mb, account, &translated(remote, &renamed_all));
                let name = if what.name.trim().is_empty() { lang.t("(no name)").to_string() } else { shown(&what.name) };
                let line = lang.tn(
                    u64::from(*items),
                    "The folder \"{name}\" wasn't deleted: the server has {n} new item in it that you hadn't seen. It comes back with it; delete it again if you still want it gone.",
                    "The folder \"{name}\" wasn't deleted: the server has {n} new items in it that you hadn't seen. It comes back with them; delete it again if you still want it gone.",
                );
                errors.push(mailcraft_i18n::fmt(line, &[("name", &name), ("n", &items.to_string())]));
            }
            (ImapFolderChange::Delete { remote, .. }, ImapFolderOutcome::Refused { why }) => {
                // The server keeps it: the listing brings it back here.
                drop_delete(mb, account, &translated(remote, &renamed_all));
                errors.push(format!("Couldn't delete a folder on the server: {why}"));
            }
        }
    }
}

/// `path` as it is after the renames `done`.
fn translated(path: &str, done: &[(String, String)]) -> String {
    let mut path = path.to_string();
    for (from, to) in done {
        if path == *from {
            path.clone_from(to);
        }
    }
    path
}

/// A mailbox of `account` has a new path: the folder that follows it, the changes of mail waiting
/// for it and a deletion waiting for it name it by that from now on.
fn follow_rename(mb: &mut Mailbox, account: Id, r: &MailboxRenamed) {
    if r.from == r.to {
        return;
    }
    for f in mb.folders.iter_mut().filter(|f| f.account == account && f.remote == r.from) {
        f.remote.clone_from(&r.to);
    }
    for p in &mut mb.pending {
        if let Pending::Move { account: a, from, .. } | Pending::Delete { account: a, from, .. } = p
            && *a == account
            && *from == r.from
        {
            from.clone_from(&r.to);
        }
    }
    for q in &mut mb.queued {
        if let Queued::DeleteMailbox { account: a, remote, .. } = q
            && *a == account
            && *remote == r.from
        {
            remote.clone_from(&r.to);
        }
    }
}

fn drop_move(mb: &mut Mailbox, account: Id, folder: Id) {
    mb.queued.retain(|q| *q != Queued::MoveFolder { account, folder });
}

fn drop_delete(mb: &mut Mailbox, account: Id, remote: &str) {
    mb.queued.retain(|q| !matches!(q, Queued::DeleteMailbox { account: a, remote: r, .. } if *a == account && r == remote));
}

/// A folder's name as the owner reads it, for a line about it.
fn shown(name: &str) -> String {
    name.chars().filter(|c| !c.is_control()).take(80).collect()
}
