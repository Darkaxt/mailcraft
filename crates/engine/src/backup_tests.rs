// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backups (v0.12.9): a file per past year and per month of this one, written again only when its
//! mail changed, and the whole mailbox put back together from them.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use mailcraft_model::{Address, Attachment, Body, Mailbox, Message};
use mailcraft_time::Date;

use crate::backup::{self, Job, MANIFEST, STATES_KEPT, period_of};
use crate::store;

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("mailcraft-backup-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Backups")).unwrap();
        Temp(dir)
    }

    fn file(&self) -> PathBuf {
        self.0.join("mailbox.mailcraft")
    }

    fn folder(&self) -> PathBuf {
        self.0.join("Backups")
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ts(year: i32, month: u8, day: u8) -> i64 {
    Date::new(year, month, day).unwrap().to_days() * 86_400 + 12 * 3600
}

/// A mailbox saved at `path` and opened again, so that what is large stays on disk: mail of 2019,
/// of March and of November 2026, with bodies and a file over the size kept outside.
fn saved(path: &std::path::Path) -> Mailbox {
    let mut mb = Mailbox::with_local_account("Avery Quinn", "avery@example.org");
    let inbox = mb.inbox().unwrap();
    for (i, when) in [ts(2019, 6, 1), ts(2026, 3, 10), ts(2026, 11, 2)].into_iter().enumerate() {
        mb.add_message(Message {
            folder: inbox,
            subject: format!("Message {i}"),
            from: Address::new("Blake Rivera", "blake@example.org"),
            to: vec![Address::new("Avery Quinn", "avery@example.org")],
            date: when,
            received: when,
            body: Body { text: format!("Body {i} ").repeat(400), html: Some(format!("<p>Body {i}</p>").repeat(300)) },
            attachments: vec![Attachment {
                name: format!("file-{i}.bin"),
                mime: "application/octet-stream".into(),
                data: Arc::new((0..5000u32).map(|n| (n * 7 + i as u32) as u8).collect()),
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
    }
    store::save(path, &mb).unwrap();
    store::open(path).unwrap().mailbox
}

fn job(mailbox: &Mailbox, tmp: &Temp, now: i64, force: bool) -> Job {
    Job { mailbox: mailbox.clone(), path: tmp.file(), folder: tmp.folder(), now, force }
}

#[test]
fn past_years_go_by_year_this_year_by_month() {
    let now = ts(2026, 11, 16);
    assert_eq!(period_of(ts(2019, 6, 1), now), "2019");
    assert_eq!(period_of(ts(2025, 12, 31), now), "2025");
    assert_eq!(period_of(ts(2026, 1, 1), now), "2026-01");
    assert_eq!(period_of(ts(2026, 11, 16), now), "2026-11");
    assert_eq!(period_of(0, now), "undated");
}

#[test]
fn a_backup_puts_the_whole_mailbox_back() {
    let tmp = Temp::new("restore");
    let mb = saved(&tmp.file());
    let report = backup::run(&job(&mb, &tmp, ts(2026, 11, 16), false)).unwrap();
    assert_eq!(report.written, ["2019", "2026-03", "2026-11"]);
    assert_eq!((report.messages, report.periods, report.incomplete), (3, 3, 0));
    for name in ["mail-2019.mailcraft.gz", "mail-2026-03.mailcraft.gz", "mail-2026-11.mailcraft.gz", "mailbox-2026-11-16.mailcraft.gz", MANIFEST] {
        assert!(tmp.folder().join(name).is_file(), "{name}");
    }
    // Each period's file is a mailbox file of its own once uncompressed.
    let gz = fs::read(tmp.folder().join("mail-2019.mailcraft.gz")).unwrap();
    let mut json = Vec::new();
    std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&gz[..]), &mut json).unwrap();
    let year = crate::io::read_mailbox(&json).unwrap();
    assert_eq!(year.messages.values().map(|m| m.subject.clone()).collect::<Vec<_>>(), ["Message 0"]);

    let target = tmp.0.join("restored.mailcraft");
    let restored = backup::restore(&tmp.folder(), &target).unwrap();
    assert_eq!((restored.messages, restored.periods), (3, 3));
    let (before, after) = (crate::io::open_path(&tmp.file()).unwrap(), crate::io::open_path(&target).unwrap());
    assert_eq!(after.messages.len(), 3);
    for (id, m) in before.messages.iter() {
        let back = after.messages.get(id).unwrap();
        assert_eq!((&back.subject, &back.body, back.received), (&m.subject, &m.body, m.received));
        assert_eq!(
            back.attachments.iter().map(|a| a.data.clone()).collect::<Vec<_>>(),
            m.attachments.iter().map(|a| a.data.clone()).collect::<Vec<_>>()
        );
    }
    // Nothing is written over.
    assert!(backup::restore(&tmp.folder(), &target).is_err());
}

#[test]
fn only_what_changed_is_written_again() {
    let tmp = Temp::new("light");
    let mut mb = saved(&tmp.file());
    backup::run(&job(&mb, &tmp, ts(2026, 11, 16), false)).unwrap();
    // A run within a day does nothing; Back up now finds nothing changed.
    assert!(backup::run(&job(&mb, &tmp, ts(2026, 11, 16) + 3600, false)).unwrap().skipped);
    assert!(backup::run(&job(&mb, &tmp, ts(2026, 11, 16) + 3600, true)).unwrap().written.is_empty());
    // A message read into memory is the same message.
    let mut loaded = mb.clone();
    store::load_all(&tmp.file(), &mut loaded).unwrap();
    assert!(backup::run(&job(&loaded, &tmp, ts(2026, 11, 16) + 7200, true)).unwrap().written.is_empty());
    // A message of 2019 marked read: that year alone.
    let id = *mb.messages.iter().find(|(_, m)| m.subject == "Message 0").unwrap().0;
    let mut m = Message::clone(mb.messages.get(&id).unwrap());
    m.read = !m.read;
    mb.messages.insert(id, Arc::new(m));
    let report = backup::run(&job(&mb, &tmp, ts(2026, 11, 17), false)).unwrap();
    assert_eq!(report.written, ["2019"]);
}

#[test]
fn the_months_become_their_year_when_it_is_over() {
    let tmp = Temp::new("year");
    let mb = saved(&tmp.file());
    backup::run(&job(&mb, &tmp, ts(2026, 11, 16), false)).unwrap();
    let report = backup::run(&job(&mb, &tmp, ts(2027, 1, 2), false)).unwrap();
    assert_eq!(report.written, ["2026"]);
    assert_eq!(report.removed, ["mail-2026-03.mailcraft.gz", "mail-2026-11.mailcraft.gz"]);
    assert!(!tmp.folder().join("mail-2026-03.mailcraft.gz").exists());
    assert!(tmp.folder().join("mail-2026.mailcraft.gz").is_file());
    let manifest = backup::read_manifest(&tmp.folder()).unwrap();
    assert_eq!(manifest.periods.keys().collect::<Vec<_>>(), ["2019", "2026"]);
}

#[test]
fn a_week_of_day_files_is_kept() {
    let tmp = Temp::new("days");
    let mb = saved(&tmp.file());
    for day in 1..=9 {
        backup::run(&job(&mb, &tmp, ts(2026, 11, day), false)).unwrap();
    }
    let manifest = backup::read_manifest(&tmp.folder()).unwrap();
    assert_eq!(manifest.states.len(), STATES_KEPT);
    assert_eq!(manifest.states.first().unwrap(), "mailbox-2026-11-03.mailcraft.gz");
    assert!(!tmp.folder().join("mailbox-2026-11-02.mailcraft.gz").exists());
    // A file the backup didn't write stays.
    fs::write(tmp.folder().join("notes.txt"), b"mine").unwrap();
    backup::run(&job(&mb, &tmp, ts(2027, 1, 2), false)).unwrap();
    assert!(tmp.folder().join("notes.txt").is_file());
}

#[test]
fn a_damaged_file_is_not_restored_from() {
    let tmp = Temp::new("damaged");
    let mb = saved(&tmp.file());
    backup::run(&job(&mb, &tmp, ts(2026, 11, 16), false)).unwrap();
    let file = tmp.folder().join("mail-2019.mailcraft.gz");
    let mut bytes = fs::read(&file).unwrap();
    let last = bytes.len() - 20;
    bytes[last] ^= 0xff;
    fs::write(&file, bytes).unwrap();
    let e = backup::restore(&tmp.folder(), &tmp.0.join("restored.mailcraft")).unwrap_err();
    assert!(e.contains("damaged"), "{e}");
}

#[test]
fn archived_mail_whole_is_said_to_be_in_the_backup() {
    let tmp = Temp::new("archived");
    let mut mb = saved(&tmp.file());
    let by_subject = |mb: &Mailbox, subject: &str| *mb.messages.iter().find(|(_, m)| m.subject == subject).unwrap().0;
    let (whole, part) = (by_subject(&mb, "Message 0"), by_subject(&mb, "Message 1"));
    for (id, unfetched) in [(whole, false), (part, true)] {
        let mut m = Message::clone(mb.messages.get(&id).unwrap());
        m.moving_out = Some(mailcraft_model::sync::MovingOut { account: 99, id: format!("S-{id}"), archive: true, ..Default::default() });
        m.unfetched = unfetched;
        mb.messages.insert(id, Arc::new(m));
    }
    let report = backup::run(&job(&mb, &tmp, ts(2026, 11, 16), false)).unwrap();
    assert_eq!(report.archived, [whole]);
    // Its file unchanged: in the backup all the same.
    let again = backup::run(&job(&mb, &tmp, ts(2026, 11, 16) + 60, true)).unwrap();
    assert!(again.written.is_empty());
    assert_eq!(again.archived, [whole]);
}

/// A backup folder that isn't there (a drive not connected) fails the backup: it isn't made
/// again, and nothing is written anywhere else (0.12.11).
#[test]
fn a_folder_that_is_not_there_is_not_made() {
    let tmp = Temp::new("missing");
    let mb = saved(&tmp.file());
    fs::remove_dir_all(tmp.folder()).unwrap();
    let why = backup::run(&job(&mb, &tmp, ts(2026, 11, 16), true)).unwrap_err();
    assert!(why.contains("isn't there"), "{why}");
    assert!(!tmp.folder().exists());
}
