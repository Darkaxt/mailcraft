// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading Microsoft Defender's report of a scanned attachment.

use super::*;

const CLEAN: &str = "Scan starting...\r\nScan finished.\r\nScanning D:\\scan\\1\\report.pdf found no threats.\r\n";

const ONE: &str = "Scan starting...\r\nScan finished.\r\nScanning D:\\scan\\1\\invoice.pdf found 1 threats.\r\n\
<===========================LIST OF DETECTED THREATS==========================>\r\n\
----------------------------- Threat information ------------------------------\r\n\
Threat                  : Trojan:PDF/Phish!MSR\r\n\
Resources               : 1 total\r\n\
    file                : D:\\scan\\1\\invoice.pdf\r\n\
-------------------------------------------------------------------------------\r\n";

const TWO: &str = "Scanning D:\\scan\\2\\a.zip found 2 threats.\r\n\
----------------------------- Threat information ------------------------------\r\n\
Threat                  : Trojan:Win32/Wacatac.B!ml\r\n\
Resources               : 1 total\r\n\
    file                : D:\\scan\\2\\a.zip->x.exe\r\n\
----------------------------- Threat information ------------------------------\r\n\
Threat                  : Virus:DOS/EICAR_Test_File\r\n\
Resources               : 1 total\r\n\
    containerfile       : D:\\scan\\2\\a.zip\r\n";

#[test]
fn a_clean_file_is_clean_only_when_it_is_still_there() {
    assert_eq!(read_report(Some(0), CLEAN, true), ScanOutcome::Clean);
    assert_eq!(read_report(Some(0), CLEAN, false), ScanOutcome::Flagged { threats: vec![REMOVED.to_string()] });
    // Exit 0 without the words is not taken for clean.
    assert!(matches!(read_report(Some(0), "", true), ScanOutcome::Failed { .. }));
}

#[test]
fn one_threat_is_named() {
    assert_eq!(read_report(Some(2), ONE, true), ScanOutcome::Flagged { threats: vec!["Trojan:PDF/Phish!MSR".to_string()] });
    // Found and then taken away: still the threat's own name.
    assert_eq!(read_report(Some(2), ONE, false), ScanOutcome::Flagged { threats: vec!["Trojan:PDF/Phish!MSR".to_string()] });
}

#[test]
fn two_threats_are_named_once_each() {
    let doubled = format!("{TWO}{TWO}");
    assert_eq!(threats(&doubled), vec!["Trojan:Win32/Wacatac.B!ml".to_string(), "Virus:DOS/EICAR_Test_File".to_string()]);
    assert_eq!(read_report(Some(2), TWO, true), ScanOutcome::Flagged { threats: threats(TWO) });
    // Code 2 with no names still flags the file.
    assert!(matches!(read_report(Some(2), "", true), ScanOutcome::Flagged { threats } if threats.len() == 1));
}

#[test]
fn a_failure_of_the_scanner_is_no_verdict() {
    let missing = "CmdTool: Failed with hr = 0x80070002. Check C:\\Users\\x\\AppData\\Local\\Temp\\MpCmdRun.log for more information\r\n";
    assert!(matches!(read_report(Some(-2147024894), missing, true), ScanOutcome::Failed { .. }));
    assert!(matches!(read_report(None, "", true), ScanOutcome::Failed { .. }));
    // The copy gone meanwhile: real-time protection took it.
    assert_eq!(read_report(Some(-2147024894), missing, false), ScanOutcome::Flagged { threats: vec![REMOVED.to_string()] });
}
