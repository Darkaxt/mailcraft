// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An attachment the owner read, scanned by Microsoft Defender (Windows): `Services::scan_file`.
//!
//! ```text
//! <cache>/MailCraft/Scans/              beside the opened attachments' folder (`opened_files`),
//!                                       never in the mailbox's own folders
//!     <process>-<time>-<n>/report.pdf   one folder per scan, removed as soon as it is over
//! ```
//!
//! - **The mailbox is never touched:** the bytes come from the window, decoded (Defender can't see
//!   inside a content file kept compressed); nothing here reads, writes, moves or removes a file
//!   of the mailbox's.
//! - **Nothing is done to the file:** `MpCmdRun.exe -Scan -ScanType 3 -File <copy>
//!   -DisableRemediation` only reports. Exit code 0 is no threat, 2 is threats found, and the
//!   `Threat : <name>` lines name them ([`read_report`]).
//! - **A copy taken away is a finding:** real-time protection may remove the copy as soon as it is
//!   written, before or while it is scanned; that is said as a threat Defender found.
//! - **Hidden and bounded:** the scanner runs without a console window, its output is capped and
//!   it is stopped after [`TIMEOUT`].

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use mailcraft_ui_egui::attachment_scan::{ScanOutcome, ScanRequest};

use crate::opened_files;

/// The longest a scan may take.
pub const TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The most output of the scanner that is read.
const MAX_OUTPUT: usize = 64 * 1024;

/// What a copy taken away by real-time protection is reported as.
pub const REMOVED: &str = "removed by Microsoft Defender real-time protection";

/// Most threats kept for one file, and the longest name kept.
const MOST_THREATS: usize = 16;
const LONGEST_NAME: usize = 200;

/// Where the scans' copies go, and the scanner, when this computer has Microsoft Defender's
/// command-line scanner (Windows only); `None` otherwise.
pub fn available() -> Option<(PathBuf, PathBuf)> {
    if !cfg!(windows) {
        return None;
    }
    let base = opened_files::base()?.parent()?.join("Scans");
    let programs = std::env::var_os("ProgramFiles").map(PathBuf::from).filter(|p| p.is_absolute())?;
    let scanner = programs.join("Windows Defender").join("MpCmdRun.exe");
    scanner.is_file().then_some((base, scanner))
}

/// Scan `request`'s bytes with `scanner`, in a folder of its own under `base`, removed afterwards.
pub fn scan(base: &Path, scanner: &Path, request: ScanRequest) -> ScanOutcome {
    let folder = match folder(base) {
        Ok(folder) => folder,
        Err(e) => return failed(format!("the copy to scan couldn't be made ({:?})", e.kind())),
    };
    let path = folder.join(mailcraft_engine::opening::file_name(&request.name));
    let written = write(&path, &request.bytes);
    let outcome = if let Err(e) = written {
        failed(format!("the copy to scan couldn't be written ({:?})", e.kind()))
    } else if !path.exists() {
        // Written, then taken away at once: real-time protection did that.
        ScanOutcome::Flagged { threats: vec![REMOVED.to_string()] }
    } else {
        match run(scanner, &path) {
            Ok((code, output)) => read_report(code, &String::from_utf8_lossy(&output), path.exists()),
            Err(why) if !path.exists() => {
                log::debug!("the scan ended early ({why}); the copy is gone");
                ScanOutcome::Flagged { threats: vec![REMOVED.to_string()] }
            }
            Err(why) => failed(why),
        }
    };
    opened_files::remove_tree(&folder, 1);
    outcome
}

fn failed(why: String) -> ScanOutcome {
    log::warn!("an attachment couldn't be scanned: {why}");
    ScanOutcome::Failed { why }
}

/// What the scanner's exit code and output say, and whether the copy was still there after it.
pub fn read_report(code: Option<i32>, output: &str, still_there: bool) -> ScanOutcome {
    if code == Some(2) {
        let mut threats = threats(output);
        if threats.is_empty() {
            threats.push("a threat Microsoft Defender didn't name".to_string());
        }
        return ScanOutcome::Flagged { threats };
    }
    if !still_there {
        return ScanOutcome::Flagged { threats: vec![REMOVED.to_string()] };
    }
    if code == Some(0) && output.contains("found no threats") {
        return ScanOutcome::Clean;
    }
    ScanOutcome::Failed {
        why: match code {
            Some(code) => format!("Microsoft Defender's scan ended with code {code}"),
            None => "Microsoft Defender's scan was stopped".to_string(),
        },
    }
}

/// The names on the `Threat : <name>` lines of the scanner's report, each once.
pub fn threats(output: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        if key.trim() != "Threat" {
            continue;
        }
        let name: String = value.trim().chars().filter(|c| !c.is_control()).take(LONGEST_NAME).collect();
        if !name.is_empty() && !out.contains(&name) && out.len() < MOST_THREATS {
            out.push(name);
        }
    }
    out
}

/// A new folder of its own for one scan.
fn folder(base: &Path) -> std::io::Result<PathBuf> {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    opened_files::private_dirs(base)?;
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let mut last = std::io::Error::other("no folder");
    for _ in 0..16 {
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let folder = base.join(format!("{}-{stamp}-{n}", std::process::id()));
        match opened_files::private_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = opened_files::new_file(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Run the scanner on `path` (a program and its arguments, never a shell), hidden, for at most
/// [`TIMEOUT`]: its exit code and its output, at most [`MAX_OUTPUT`] bytes of it.
fn run(scanner: &Path, path: &Path) -> Result<(Option<i32>, Vec<u8>), String> {
    let mut command = Command::new(scanner);
    command.args(["-Scan", "-ScanType", "3", "-File"]).arg(path).arg("-DisableRemediation");
    command.stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashes up for it.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| format!("Microsoft Defender's scanner couldn't be started ({:?})", e.kind()))?;
    let (sender, output) = mpsc::channel();
    if let Some(stdout) = child.stdout.take() {
        let spawned = std::thread::Builder::new().name("attachment scan output".into()).spawn(move || {
            let mut out = Vec::new();
            let mut stdout = stdout;
            let _ = (&mut stdout).take(MAX_OUTPUT as u64).read_to_end(&mut out);
            // The rest is read and dropped, so that the scanner never waits on a full pipe.
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            let _ = sender.send(out);
        });
        if spawned.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the scanner's output couldn't be read".into());
        }
    }
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Microsoft Defender's scan didn't finish in time".into());
            }
        }
    };
    let out = output.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    Ok((status.code(), out))
}

#[cfg(test)]
#[path = "attachment_scan_tests.rs"]
mod attachment_scan_tests;
