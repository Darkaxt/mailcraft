// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Local archive put away (0.12.30, `crate::parking`): the folders count it as before, and
//! nothing of it is lost to a save, a whole rewrite of the file, a backup or an Undo.

use std::collections::BTreeSet;

use super::*;

/// A mailbox of `n` messages (as [`mailbox`] makes them) whose messages but the first `keep` are
/// in the archive: the twin of its Inbox. The Inbox and the archive folder.
fn archived(n: usize, keep: usize) -> (Mailbox, Id, Id) {
    let mut mb = mailbox(n);
    let inbox = mb.inbox().unwrap();
    let twin = crate::cmd::archive::twin(&mut mb, inbox).unwrap();
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for id in ids.iter().skip(keep) {
        Arc::make_mut(mb.messages.get_mut(id).unwrap()).folder = twin;
    }
    (mb, inbox, twin)
}

/// What each folder holds: messages and unread.
fn held(s: &Session, folders: &[Id]) -> Vec<(usize, usize)> {
    folders.iter().map(|f| (s.mailbox.count(*f), s.mailbox.unread_count(*f))).collect()
}

#[test]
fn every_message_packed_comes_back_as_it_was() {
    let tmp = Temp::new("park-round-trip");
    let path = tmp.file();
    let (mb, _, _) = archived(24, 0);
    save(&path, &mb).unwrap();
    clear_cache();
    let s = session_without_bodies(&path);
    assert_eq!(s.mailbox.messages.len(), 24);
    for m in s.mailbox.messages.values() {
        let p = crate::parking::freeze(m).expect("packed");
        assert_eq!(crate::parking::thaw(&p).as_ref(), Some(&**m));
    }
}

#[test]
fn the_archive_put_away_is_counted_kept_by_saves_and_taken_out_whole() {
    let tmp = Temp::new("park-save");
    let path = tmp.file();
    let (mb, inbox, twin) = archived(40, 8);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let all = s.mailbox.messages.clone();
    let counts = held(&s, &[inbox, twin]);
    let folder_counts = crate::counts::FolderCounts::of(&s.mailbox);
    assert_eq!(s.park_archive(), 32);
    assert_eq!(s.mailbox.messages.len(), 8);
    assert_eq!(s.archive_parked(), 32);
    assert_eq!(held(&s, &[inbox, twin]), counts, "the folders count the archive as before");
    let parked_counts = crate::counts::FolderCounts::of(&s.mailbox);
    for f in [inbox, twin] {
        assert_eq!((parked_counts.total(f), parked_counts.unread(f)), (folder_counts.total(f), folder_counts.unread(f)));
    }
    let local = s.mailbox.local_account();
    assert_eq!(parked_counts.account(local), folder_counts.account(local));
    // The Inbox's list shows its twin: the row at its end takes the archive out.
    s.view.folder = inbox;
    assert_eq!(s.archive_parked_in_view(), 32);

    // A change elsewhere, saved: the file still has every message of the archive.
    let changed = *s.mailbox.messages.keys().next().unwrap();
    Arc::make_mut(s.mailbox.messages.get_mut(&changed).unwrap()).subject = "Changed while the archive was away".into();
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    let expected = whole(&path);
    assert_eq!(expected.messages.len(), 40);
    assert_eq!(expected.message(changed).unwrap().subject, "Changed while the archive was away");

    // The file written whole (as after another program wrote it): the archive goes in as it is.
    lock(&shared(&path).state).disk = None;
    let saved = save(&path, &s.mailbox).unwrap();
    assert!(saved.compacted);
    clear_cache();
    let after = whole(&path);
    assert!(after.messages == expected.messages, "a whole rewrite keeps the archive");

    // Taken out: every message as it was.
    assert_eq!(s.load_archive(), 32);
    assert_eq!(s.archive_parked(), 0);
    assert_eq!(s.mailbox.messages.len(), 40);
    for (id, m) in all.iter() {
        if *id != changed {
            assert_eq!(s.mailbox.message(*id), Some(&**m));
        }
    }
    // And the file is known to have them: the next save writes nothing for them.
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    assert!(whole(&path).messages == expected.messages);
}

#[test]
fn an_undo_after_the_archive_went_away_puts_back_only_what_the_step_changed() {
    let tmp = Temp::new("park-undo");
    let path = tmp.file();
    let (mb, inbox, twin) = archived(20, 5);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let archived_one = s.mailbox.messages.values().find(|m| m.folder == twin).unwrap().id;
    s.run("mail.markRead", &json!({"id": archived_one})).unwrap();
    assert!(s.mailbox.message(archived_one).unwrap().read);
    save(&path, &s.mailbox).unwrap();
    let before_park = held(&s, &[inbox, twin]);
    assert_eq!(s.park_archive(), 15);
    assert_eq!(held(&s, &[inbox, twin]), before_park);
    // The step lets go of what it didn't change, sharing the rest with the mailbox.
    let step = &s.history.last().unwrap().mailbox;
    let kept: BTreeSet<Id> = step.messages.keys().copied().collect();
    let resident: BTreeSet<Id> = s.mailbox.messages.keys().copied().chain([archived_one]).collect();
    assert_eq!(kept, resident);

    s.run("edit.undo", &json!({})).unwrap();
    let m = s.mailbox.message(archived_one).expect("put back");
    assert!(!m.read, "the step is undone");
    assert!(!s.mailbox.parked.contains(&archived_one));
    assert_eq!(s.archive_parked(), 14);
    assert_eq!(s.mailbox.count(twin), 15);
    assert_eq!(s.mailbox.unread_count(twin), 15);
    // Nothing of the archive is lost to it: taken out, every message is there.
    s.load_archive();
    assert_eq!(s.mailbox.messages.len(), 20);
}

#[test]
fn a_command_naming_a_message_put_away_takes_the_archive_out_first() {
    let tmp = Temp::new("park-command");
    let path = tmp.file();
    let (mb, _, twin) = archived(12, 2);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let one = s.mailbox.messages.values().find(|m| m.folder == twin).unwrap().id;
    assert_eq!(s.park_archive(), 10);
    s.run("mail.markRead", &json!({"id": one})).unwrap();
    assert_eq!(s.archive_parked(), 0);
    assert!(s.mailbox.message(one).unwrap().read);
    // Undone, it is unread again: the step saw the archive out.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(!s.mailbox.message(one).unwrap().read);
    assert_eq!(s.mailbox.messages.len(), 12);
}

#[test]
fn a_backup_holds_the_archive_put_away() {
    let tmp = Temp::new("park-backup");
    let path = tmp.file();
    let (mb, _, _) = archived(16, 4);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    assert_eq!(s.park_archive(), 12);
    let folder = tmp.0.join("backups");
    fs::create_dir_all(&folder).unwrap();
    let job = crate::backup::Job { mailbox: s.mailbox.clone(), path: path.clone(), folder, now: 1_800_000_000, force: true };
    let report = crate::backup::run(&job).unwrap();
    assert_eq!(report.messages, 16);
}

#[test]
fn only_what_the_file_has_as_it_is_goes_away() {
    let tmp = Temp::new("park-unsaved");
    let path = tmp.file();
    let (mb, _, twin) = archived(10, 0);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    let one = s.mailbox.messages.values().find(|m| m.folder == twin).unwrap().id;
    Arc::make_mut(s.mailbox.messages.get_mut(&one).unwrap()).subject = "Not saved yet".into();
    assert_eq!(s.park_archive(), 9);
    assert_eq!(s.mailbox.message(one).unwrap().subject, "Not saved yet");
    save(&path, &s.mailbox).unwrap();
    assert_eq!(s.park_archive(), 1);
    assert_eq!(s.mailbox.messages.len(), 0);
    clear_cache();
    assert_eq!(whole(&path).message(one).unwrap().subject, "Not saved yet");
}
