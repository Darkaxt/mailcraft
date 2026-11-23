// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a window holds once the Local archive is put away (0.12.33), on a copy of a real mailbox
//! file. Nothing is written: the file can be read-only.
//!
//! `cargo run -p mailcraft-engine --example archive_open_bench -- FILE [--before] [--compare]`
//!
//! As now: the archive put away as the file is read (`store::open_for_window`). `--before`: as
//! 0.12.32 did, everything read first, then the archive put away. Either way, what is left of the
//! archive is put away after, as the window does, and the process's memory is said (private bytes
//! and working set). `--compare`: then every message put away is taken out and checked against
//! the same file read without putting anything away. Totals only: nothing of any message is said.

use std::path::PathBuf;
use std::time::Instant;

use mailcraft_engine::Session;
use mailcraft_engine::io::OpenProgress;
use mailcraft_engine::store;

/// Private bytes and working set, MiB (Windows; elsewhere nothing is said).
#[cfg(windows)]
fn memory() -> (u64, u64) {
    let mut sys = sysinfo::System::new().expect("system");
    let pid = sysinfo::get_current_pid().expect("pid");
    sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]), false, sysinfo::ProcessRefreshKind::nothing().with_memory());
    let p = sys.process(pid).expect("this process");
    (p.virtual_memory() >> 20, p.memory() >> 20)
}

#[cfg(not(windows))]
fn memory() -> (u64, u64) {
    (0, 0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = PathBuf::from(args.iter().find(|a| !a.starts_with("--")).expect("a mailbox file"));
    let before = args.iter().any(|a| a == "--before");
    let compare = args.iter().any(|a| a == "--compare");
    let started = Instant::now();
    let opened =
        if before { store::open_for_headless(&path, &OpenProgress::default()) } else { store::open_for_window(&path, &OpenProgress::default()) }
            .expect("opened");
    let opened_ms = started.elapsed().as_millis();
    let (open_private, open_ws) = memory();
    let at_open = opened.mailbox.parked.len();
    let repaired = opened.repaired;
    let mut s = Session::new(opened.mailbox);
    s.path = Some(path.clone());
    let after = s.tend_archive(0.0);
    let (private, ws) = memory();
    println!(
        "{}: opened in {opened_ms} ms (repaired {repaired}); {} messages stay, {at_open} put away at open and {after} after ({} KB packed); private {open_private} MiB / working set {open_ws} MiB once open, {private} / {ws} MiB once put away",
        if before { "as 0.12.32" } else { "as now" },
        s.mailbox.messages.len(),
        s.mailbox.parked.bytes() / 1024
    );
    if compare {
        let away: Vec<_> = s.mailbox.parked.ids().collect();
        let stay = s.mailbox.messages.len();
        assert_eq!(s.load_archive(), away.len());
        let read = store::open_for_headless(&path, &OpenProgress::default()).expect("read").mailbox;
        let differ = away.iter().filter(|id| read.message(**id) != s.mailbox.message(**id)).count();
        let missing = read.messages.len().saturating_sub(away.len() + stay);
        println!("compared {} put away with the file read as it is: {differ} differ, {missing} missing", away.len());
        assert_eq!((differ, missing), (0, 0));
    }
}
