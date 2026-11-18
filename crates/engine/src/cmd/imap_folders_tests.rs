// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP account's folders renamed, moved and deleted here go to its server (v0.12.14): what a
//! run is asked to do, what waits, and what is taken in of what the server did.

use mailcraft_model::sync::{
    ImapFolderChange, ImapFolderDone, ImapFolderOutcome, ListedFolder, MailboxRenamed, RemoteFlags, RemoteOp, Shown, SyncResult,
};
use mailcraft_model::{AccountKind, FolderKind, Id, Message, Pending};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};

const EMAIL: &str = "ada@imap.example";

/// A session with an IMAP account whose Inbox and Deleted Items follow INBOX and Trash, and
/// whose folder "Projects" holds "Apollo", with a message the server has as UID 5 there; the
/// account, "Projects", "Apollo" and the message.
fn session() -> (Session, Id, Id, Id, Id) {
    let mut s = Session::empty();
    let servers = json!({"email": EMAIL, "incoming": {"host": "imap.example.org"}, "outgoing": {"host": "smtp.example.org"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.account(account).map(|a| a.kind), Some(AccountKind::Imap));
    let projects = s.mailbox.add_folder(account, None, "Projects").unwrap();
    let apollo = s.mailbox.add_folder(account, Some(projects), "Apollo").unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    for (id, remote, uid_validity) in [(inbox, "INBOX", 7), (deleted, "Trash", 8), (projects, "Projects", 31), (apollo, "Projects/Apollo", 32)] {
        let f = s.mailbox.folder_mut(id).unwrap();
        (f.remote, f.uid_validity) = (remote.into(), uid_validity);
    }
    let synced = Some(RemoteFlags { uid: 5, ..Default::default() });
    let plans = Message { folder: apollo, uid: 5, synced, subject: "Plans".into(), ..Default::default() };
    let plans = s.mailbox.add_message(plans).unwrap();
    (s, account, projects, apollo, plans)
}

fn result(account: Id, folders_changed: Vec<ImapFolderDone>, listed: Vec<ListedFolder>) -> SyncResult {
    SyncResult { account, email: EMAIL.into(), folders_changed, listed, ..Default::default() }
}

fn listed(remote: &str, parent: &str, name: &str) -> ListedFolder {
    ListedFolder { remote: remote.into(), kind: FolderKind::Custom, parent: parent.into(), name: name.into() }
}

/// A folder renamed here is asked of the next run as a rename of its mailbox; a folder deleted
/// here waits until the mail moved out of its mailbox is out, its mailbox kept for those moves,
/// and the rename of the mailbox above it waits with it (v0.12.14).
#[test]
fn a_run_renames_at_once_and_deletes_once_the_mail_is_out() {
    let (mut s, account, projects, apollo, _) = session();
    s.mailbox.rename_folder(projects, "Work").unwrap();
    let request = sync_request(&s, account).unwrap();
    assert_eq!(
        request.folder_changes,
        [ImapFolderChange::Rename { folder: projects, from: "Projects".into(), parent: String::new(), name: "Work".into() }]
    );

    assert_eq!(s.mailbox.delete_folder(apollo), Ok(false));
    let request = sync_request(&s, account).unwrap();
    // The move out goes first, from the mailbox no folder here follows any more.
    assert!(
        request.ops.iter().any(|op| matches!(op, RemoteOp::Move { from, to, uid: 5, .. } if from == "Projects/Apollo" && to == "Trash")),
        "{:?}",
        request.ops
    );
    assert!(request.folders.iter().any(|f| f.remote == "Projects/Apollo" && f.uid_validity == 32));
    assert!(request.folder_changes.is_empty(), "{:?}", request.folder_changes);

    // (As if a run had moved it.)
    s.mailbox.pending.clear();
    let request = sync_request(&s, account).unwrap();
    assert_eq!(
        request.folder_changes,
        [
            ImapFolderChange::Rename { folder: projects, from: "Projects".into(), parent: String::new(), name: "Work".into() },
            ImapFolderChange::Delete { remote: "Projects/Apollo".into(), shown: Shown::of("Apollo", [], []) },
        ]
    );
}

/// A rename the server made has the folders here follow the new paths, those under it too, and
/// what waits for their mailboxes with them; it is not asked again (v0.12.14).
#[test]
fn a_rename_the_server_made_is_followed_here() {
    let (mut s, account, projects, apollo, plans) = session();
    let inbox = s.mailbox.inbox().unwrap();
    s.mailbox.rename_folder(projects, "Work").unwrap();
    let change = sync_request(&s, account).unwrap().folder_changes.remove(0);
    // Moved while the run was under way: it waits for the mailbox by its old path.
    s.mailbox.move_message(plans, inbox).unwrap();
    let renamed = [("Projects", "Work"), ("Projects/Apollo", "Work/Apollo")]
        .iter()
        .map(|(from, to)| MailboxRenamed { from: (*from).into(), to: (*to).into() })
        .collect();
    let done = ImapFolderDone { change, outcome: ImapFolderOutcome::Done { renamed } };
    // The server lists them by their new paths.
    apply_sync(&mut s, result(account, vec![done], vec![listed("Work", "", "Work"), listed("Work/Apollo", "Work", "Apollo")])).unwrap();
    assert_eq!(s.mailbox.folder(projects).map(|f| (f.remote.as_str(), f.name.as_str())), Some(("Work", "Work")));
    assert_eq!(s.mailbox.folder(apollo).map(|f| f.remote.as_str()), Some("Work/Apollo"));
    assert!(matches!(&s.mailbox.pending[..], [Pending::Move { from, uid: 5, .. }] if from == "Work/Apollo"), "{:?}", s.mailbox.pending);
    assert!(s.mailbox.queued.is_empty());
    // No folder was made of the new paths.
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == account && f.remote.starts_with("Work")).count(), 2);
}

/// A mailbox waiting to be deleted is not taken for a new folder when the server still lists it;
/// one the server kept for mail the owner hadn't seen is said, and comes back (v0.12.14).
#[test]
fn a_mailbox_waiting_to_be_deleted_is_not_brought_back_until_the_server_keeps_it() {
    let (mut s, account, projects, _, _) = session();
    s.mailbox.delete_folder(projects).unwrap();
    s.mailbox.pending.clear();
    let still = || vec![listed("Projects", "", "Projects"), listed("Projects/Apollo", "Projects", "Apollo")];
    apply_sync(&mut s, result(account, Vec::new(), still())).unwrap();
    assert!(!s.mailbox.folders.iter().any(|f| f.remote.starts_with("Projects")));

    let changes = sync_request(&s, account).unwrap().folder_changes;
    assert_eq!(changes.len(), 2);
    let done: Vec<ImapFolderDone> = changes
        .into_iter()
        .map(|change| {
            let outcome = match &change {
                ImapFolderChange::Delete { remote, .. } if remote == "Projects" => ImapFolderOutcome::NotShown { items: 2 },
                _ => ImapFolderOutcome::Done { renamed: Vec::new() },
            };
            ImapFolderDone { change, outcome }
        })
        .collect();
    let answer = apply_sync(&mut s, result(account, done, vec![listed("Projects", "", "Projects")])).unwrap();
    let errors = answer["errors"].to_string();
    assert!(errors.contains("Projects") && errors.contains("2 new items"), "{errors}");
    assert!(s.mailbox.queued.is_empty());
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == account && f.remote == "Projects").count(), 1);
}
