// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real owned-tree regression: the root exits while its child awaits pipe input.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

use super::{Cancellation, ManagedProcess};

#[test]
#[ignore = "inert child-process fixture, invoked only by the owned-tree tests"]
#[allow(
    clippy::zombie_processes,
    reason = "the fixture must exit before its descendant; the test's exact owned job retains and observes that descendant"
)]
fn tree_fixture() {
    let role = std::env::var("CRAFT_PROCESS_TREE_FIXTURE").unwrap();
    let mut byte = [0];
    std::io::stdin().read_exact(&mut byte).unwrap();
    if role == "descendant" {
        assert_eq!(byte, [b'X']);
        return;
    }
    assert_eq!(byte, [b'S']);
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "process::tests::tree_fixture", "--ignored", "--nocapture"])
        .env("CRAFT_PROCESS_TREE_FIXTURE", "descendant")
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    println!("craft-descendant-ready:{}", child.id());
    std::io::stdout().flush().unwrap();
    // Intentionally leave the child blocked on the inherited input pipe: only
    // the test's exact owned job or its explicit X input may end this fixture.
}

fn exercise(cancel: bool) {
    let cancellation = Cancellation::default();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process::tests::tree_fixture", "--ignored", "--nocapture"])
        .env("CRAFT_PROCESS_TREE_FIXTURE", "root")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut process = ManagedProcess::spawn(command, &cancellation).unwrap();
    let mut input = process.stdin().unwrap();
    input.write_all(b"S").unwrap();
    input.flush().unwrap();
    let mut output = BufReader::new(process.stdout().unwrap());
    loop {
        let mut line = String::new();
        assert_ne!(output.read_line(&mut line).unwrap(), 0, "root exited before reporting its child");
        if line.contains("craft-descendant-ready:") {
            break;
        }
    }
    assert!(process.child.inner_mut().wait().unwrap().success());
    // A non-mutating kernel-state probe: expiry neither cancels the process nor
    // determines completion. The descendant still owns a live input wait.
    assert_eq!(process.completion_job.wait_tree_timeout(std::time::Duration::ZERO).unwrap(), cosca::containment::TreeDrain::MembersRemain);
    if cancel {
        cancellation.cancel();
    } else {
        input.write_all(b"X").unwrap();
        input.flush().unwrap();
    }
    assert!(process.wait().unwrap().success());
    assert_eq!(process.completion_job.wait_tree().unwrap(), cosca::containment::TreeDrain::AllMembersExited);
    assert!(process.watcher.is_none());
}

#[test]
fn content_tree_waits_for_descendant_after_root_exit() {
    exercise(false);
}

#[test]
fn content_tree_cancellation_retains_exit_observation() {
    exercise(true);
}
