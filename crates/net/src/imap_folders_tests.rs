// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP account's folders renamed, moved and deleted here, made on the fake server (v0.12.14).

use mailcraft_model::sync::{ImapFolderChange, ImapFolderDone, ImapFolderOutcome, MailboxRenamed, Shown};

use super::*;

/// A folder here that follows the mailbox `remote`.
fn followed(id: Id, remote: &str, uid_validity: u32) -> FolderState {
    FolderState { id, remote: remote.into(), kind: FolderKind::Custom, uid_validity, ..Default::default() }
}

fn rename(folder: Id, from: &str, parent: &str, name: &str) -> ImapFolderChange {
    ImapFolderChange::Rename { folder, from: from.into(), parent: parent.into(), name: name.into() }
}

fn delete(remote: &str, shown: Shown) -> ImapFolderChange {
    ImapFolderChange::Delete { remote: remote.into(), shown }
}

fn renamed(pairs: &[(&str, &str)]) -> ImapFolderOutcome {
    ImapFolderOutcome::Done { renamed: pairs.iter().map(|(from, to)| MailboxRenamed { from: (*from).into(), to: (*to).into() }).collect() }
}

/// Run `changes` against `world`: what became of each, the commands that changed the server, and
/// the server's mailboxes after.
fn changed(world: ImapWorld, folders: Vec<FolderState>, changes_asked: Vec<ImapFolderChange>) -> (Vec<ImapFolderOutcome>, Vec<String>, Vec<String>) {
    let imap = Fake::imap(world);
    let mut folders_asked = local_folders();
    folders_asked.extend(folders);
    let asked = SyncRequest { folder_changes: changes_asked.clone(), ..request(account(Some(&imap), None), folders_asked) };
    let result = sync(&asked);
    assert_eq!(result.errors, Vec::<String>::new());
    let done: Vec<ImapFolderChange> = result.folders_changed.iter().map(|d: &ImapFolderDone| d.change.clone()).collect();
    assert_eq!(done, changes_asked);
    let outcomes = result.folders_changed.into_iter().map(|d| d.outcome).collect();
    let mailboxes = imap.world().mailboxes.iter().map(|m| m.raw.clone()).collect();
    (outcomes, changes(&imap), mailboxes)
}

/// A folder renamed and moved here has its mailbox renamed on the server, with those under it
/// along, and every path that changed is said (v0.12.14).
#[test]
fn a_folder_renamed_here_is_renamed_on_the_server_with_its_subfolders() {
    let mut world = imap_world();
    world.mailboxes.push(mailbox("Projects/Apollo", "\\HasNoChildren", 32, vec![message(1, "", "Apollo one")]));
    let (outcomes, commands, mailboxes) =
        changed(world, vec![followed(1, "Projects", 31), followed(2, "Projects/Apollo", 32)], vec![rename(1, "Projects", "", "Work")]);
    assert_eq!(outcomes, [renamed(&[("Projects", "Work"), ("Projects/Apollo", "Work/Apollo")])]);
    assert_eq!(commands, ["RENAME \"Projects\" \"Work\"", "SUBSCRIBE \"Work\""]);
    assert!(mailboxes.contains(&"Work".to_string()) && mailboxes.contains(&"Work/Apollo".to_string()));
    assert!(!mailboxes.iter().any(|m| m.starts_with("Projects")));
}

/// On Gmail, a folder moved out of the Trash to the top level leaves `[Gmail]/Trash` on the
/// server, and one moved into the Trash is never renamed there (v0.12.14).
#[test]
fn a_folder_leaves_the_gmail_trash_and_none_is_renamed_into_it() {
    let mut world = imap_world();
    world.mailboxes.push(mailbox("[Gmail]/Trash", "\\HasChildren \\Trash", 61, Vec::new()));
    world.mailboxes.push(mailbox("[Gmail]/Trash/Old labels", "\\HasNoChildren", 62, Vec::new()));
    let (outcomes, commands, mailboxes) = changed(
        world,
        vec![followed(1, "[Gmail]/Trash/Old labels", 62), followed(2, "Projects", 31)],
        vec![rename(1, "[Gmail]/Trash/Old labels", "", "Old labels"), rename(2, "Projects", "[Gmail]/Trash", "Projects")],
    );
    assert_eq!(outcomes[0], renamed(&[("[Gmail]/Trash/Old labels", "Old labels")]));
    assert!(matches!(&outcomes[1], ImapFolderOutcome::Refused { .. }), "{outcomes:?}");
    assert_eq!(commands, ["RENAME \"[Gmail]/Trash/Old labels\" \"Old labels\"", "SUBSCRIBE \"Old labels\""]);
    assert!(!mailboxes.iter().any(|m| m.starts_with("[Gmail]/Trash/")), "{mailboxes:?}");
}

/// A folder deleted here has its mailbox deleted on the server when it holds nothing beyond
/// what the owner had of it; one already gone is said to be (v0.12.14).
#[test]
fn a_folder_deleted_here_is_deleted_on_the_server_when_nothing_unseen_is_in_it() {
    let mut world = imap_world();
    world.mailboxes.push(mailbox("Old", "\\HasNoChildren", 71, Vec::new()));
    world.mailboxes.push(mailbox("Read", "\\HasNoChildren", 72, vec![message(3, "\\Seen", "Kept"), message(4, "\\Deleted", "Going")]));
    let seen = Shown::imap_message(72, 3);
    let (outcomes, commands, mailboxes) = changed(
        world,
        Vec::new(),
        vec![delete("Old", Shown::default()), delete("Read", Shown::of("Read", [], [seen.as_str()])), delete("Nowhere", Shown::default())],
    );
    assert_eq!(outcomes, [renamed(&[]), renamed(&[]), ImapFolderOutcome::Gone]);
    assert_eq!(commands, ["DELETE \"Old\"", "DELETE \"Read\""]);
    assert!(!mailboxes.iter().any(|m| m == "Old" || m == "Read"));
}

/// A mailbox holding mail the owner hadn't seen here, or a mailbox under it, is not deleted
/// (AU3-H7): how many such items it holds is said, and no DELETE is sent (v0.12.14).
#[test]
fn a_mailbox_with_unseen_mail_or_subfolders_is_kept() {
    let mut world = imap_world();
    world.mailboxes.push(mailbox("Busy", "\\HasNoChildren", 73, vec![message(1, "", "New"), message(2, "\\Deleted", "Gone")]));
    world.mailboxes.push(mailbox("Parent", "\\HasChildren", 74, Vec::new()));
    world.mailboxes.push(mailbox("Parent/Child", "\\HasNoChildren", 75, Vec::new()));
    let (outcomes, commands, mailboxes) = changed(world, Vec::new(), vec![delete("Busy", Shown::default()), delete("Parent", Shown::default())]);
    assert_eq!(outcomes, [ImapFolderOutcome::NotShown { items: 1 }, ImapFolderOutcome::NotShown { items: 1 }]);
    assert_eq!(commands, Vec::<String>::new());
    assert!(["Busy", "Parent", "Parent/Child"].iter().all(|m| mailboxes.contains(&m.to_string())));
}

/// INBOX, the server's own mailboxes (RFC 6154) and those holding one are never renamed or
/// deleted from here (v0.12.14).
#[test]
fn the_servers_own_mailboxes_are_never_renamed_or_deleted() {
    let (outcomes, commands, _) = changed(
        imap_world(),
        Vec::new(),
        // (The renames are made first, whatever the order asked.)
        vec![
            rename(1, "[Gmail]", "", "Google"),
            rename(2, "[Gmail]/All Mail", "", "Everything"),
            delete("Sent Items", Shown::default()),
            delete("INBOX", Shown::default()),
        ],
    );
    assert!(outcomes.iter().all(|o| matches!(o, ImapFolderOutcome::Refused { .. })), "{outcomes:?}");
    assert_eq!(commands, Vec::<String>::new());
}
