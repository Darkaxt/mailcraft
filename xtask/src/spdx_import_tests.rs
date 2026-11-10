// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{HEADER, expected_header, missing};

#[test]
fn vendored_process_dependency_retains_its_actual_apache_copyright() {
    let path = "vendor/cosca/src/elevation/windows.rs";
    let header = "// SPDX-FileCopyrightText: Anna Zhukova\n// SPDX-License-Identifier: Apache-2.0\n";
    assert_eq!(expected_header(path), header);
    assert!(missing(&[path.to_owned()], |_| Some(header.into())).is_empty());
    assert_eq!(missing(&[path.to_owned()], |_| Some(HEADER.into())), [path]);
}

#[test]
fn imported_adapters_require_their_actual_copyright_and_mit_licence() {
    let external = "native/craft-content/adapters/wordcraft/engine.rs";
    let modified = "native/craft-content/adapters/gridcraft/engine.rs";
    let ordinary = "crates/model/src/lib.rs";
    let files = [external, modified, ordinary].map(str::to_owned);
    assert!(missing(&files, |path| Some(expected_header(path).replace('\n', "\r\n"))).is_empty());
    assert_eq!(missing(&files, |_| Some(HEADER.into())), [external, modified]);
    assert_eq!(missing(&files, |_| Some(expected_header(external).into())), [modified, ordinary]);
    assert_eq!(missing(&files, |path| Some(expected_header(path).replace("MIT", "GPL-3.0-only"))), [external, modified, ordinary]);
    let other = ["native/craft-content/adapters/unlisted/engine.rs".to_owned()];
    assert_eq!(missing(&other, |_| Some(expected_header(external).into())), [other[0].as_str()]);
}
