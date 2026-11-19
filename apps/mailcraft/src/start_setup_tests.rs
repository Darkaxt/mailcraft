// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn folder() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn mailbox_in(dir: &Path) -> PathBuf {
    let mailbox = dir.join("mailbox.mailcraft");
    std::fs::write(&mailbox, b"mail").unwrap();
    mailbox
}

/// The owner's start is kept and a plain start takes it back: the mailbox, the control port,
/// agents-only, the Google registration's file and the temporary folder.
#[test]
fn a_start_with_a_mailbox_is_taken_back_by_a_plain_start() {
    let (settings, data) = (folder(), folder());
    assert_eq!(recall(settings.path()), None, "nothing kept yet: a first start stays as it was");
    let setup = Setup {
        mailbox: mailbox_in(data.path()),
        control: Some(7991),
        agents_only: true,
        google_file: Some(data.path().join("google-desktop.json")),
        temp: Some(data.path().join("temp")),
    };
    remember(settings.path(), &setup);
    assert_eq!(recall(settings.path()), Some(setup.clone()));
    // Kept again unchanged, the file isn't rewritten.
    let before = std::fs::metadata(settings.path().join(FILE)).unwrap().modified().unwrap();
    remember(settings.path(), &setup);
    assert_eq!(std::fs::metadata(settings.path().join(FILE)).unwrap().modified().unwrap(), before);
    // Without control, as kept.
    let plain = Setup { mailbox: setup.mailbox.clone(), ..Default::default() };
    remember(settings.path(), &plain);
    assert_eq!(recall(settings.path()), Some(plain));
}

/// A mailbox that is no longer there, a relative path, a port out of range or another schema:
/// nothing is taken back, so the plain start stays what it always was.
#[test]
fn what_cant_be_used_is_not_taken_back() {
    let (settings, data) = (folder(), folder());
    let mailbox = mailbox_in(data.path());
    remember(settings.path(), &Setup { mailbox: PathBuf::from("relative.mailcraft"), ..Default::default() });
    assert!(!settings.path().join(FILE).exists(), "a relative mailbox isn't kept");
    remember(settings.path(), &Setup { mailbox: mailbox.clone(), ..Default::default() });
    std::fs::remove_file(&mailbox).unwrap();
    assert_eq!(recall(settings.path()), None, "the mailbox file is gone");
    let mailbox = mailbox_in(data.path());
    let write = |v: Value| std::fs::write(settings.path().join(FILE), serde_json::to_vec(&v).unwrap()).unwrap();
    write(json!({"schema": 1, "mailbox": mailbox, "control": 70000}));
    assert_eq!(recall(settings.path()), None);
    write(json!({"schema": 2, "mailbox": mailbox}));
    assert_eq!(recall(settings.path()), None);
    write(json!({"schema": 1, "mailbox": mailbox, "googleClientFile": "relative.json", "temp": "relative"}));
    let kept = recall(settings.path()).unwrap();
    assert_eq!((kept.google_file, kept.temp), (None, None), "relative paths aren't taken back");
    remember(settings.path(), &Setup { mailbox: mailbox.clone(), temp: Some(PathBuf::from("tmp")), ..Default::default() });
    assert_eq!(recall(settings.path()).unwrap().temp, None, "a relative temporary folder isn't kept");
    std::fs::write(settings.path().join(FILE), b"not json").unwrap();
    assert_eq!(recall(settings.path()), None);
}

/// `start.json` names the Google file and never holds what is in it.
#[test]
fn the_kept_start_holds_no_secret() {
    let (settings, data) = (folder(), folder());
    let google = data.path().join("google-desktop.json");
    let secret = "GOCSPX-not-a-real-secret";
    std::fs::write(
        &google,
        serde_json::to_vec(&json!({"installed": {"client_id": "123-abc.apps.googleusercontent.com", "client_secret": secret}})).unwrap(),
    )
    .unwrap();
    remember(settings.path(), &Setup { mailbox: mailbox_in(data.path()), google_file: Some(google.clone()), ..Default::default() });
    let kept = std::fs::read_to_string(settings.path().join(FILE)).unwrap();
    assert!(!kept.contains(secret) && !kept.contains("123-abc"), "{kept}");
    assert_eq!(google_client(&google), Some(("123-abc.apps.googleusercontent.com".into(), secret.into())));
}

/// A registration file that isn't Google's installed-app JSON, or whose id or secret has what an
/// id or a secret can't have, gives nothing.
#[test]
fn a_registration_file_is_checked() {
    let data = folder();
    let file = data.path().join("google.json");
    for bad in [
        json!({"web": {"client_id": "1-a.apps.googleusercontent.com", "client_secret": "s"}}),
        json!({"installed": {"client_id": "bad id", "client_secret": "s"}}),
        json!({"installed": {"client_id": "1-a.apps.googleusercontent.com", "client_secret": "has space"}}),
        json!({"installed": {"client_id": "", "client_secret": "s"}}),
    ] {
        std::fs::write(&file, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert_eq!(google_client(&file), None, "{bad}");
    }
    assert_eq!(google_client(&data.path().join("missing.json")), None);
}
