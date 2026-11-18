// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use serde_json::json;

use super::native::locate;

/// Pages count across a document's parts (sheets, slides), and a page past the end is no page
/// (v0.12.14).
#[test]
fn pages_are_found_across_parts() {
    let info = json!({"parts": [{"pages": 2}, {"pages": 3}]});
    assert_eq!(locate(&info, 1), Ok((0, 0, 5)));
    assert_eq!(locate(&info, 3), Ok((1, 0, 5)));
    assert_eq!(locate(&info, 5), Ok((1, 2, 5)));
    assert!(locate(&info, 6).is_err());
    assert!(locate(&json!({}), 1).is_err());
}
