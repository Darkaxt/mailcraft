// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn inspection_visits_every_alternative_instead_of_picking_a_display_body() {
    let input = b"Content-Type: multipart/alternative; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nplain\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>hidden alternative</p>\r\n--b--\r\n";
    let mut leaves = Vec::new();
    let report = visit(input, 1000, |leaf| {
        leaves.push((leaf.path, leaf.mime, leaf.decoded));
        Ok(())
    })
    .unwrap();
    assert_eq!(leaves.len(), 2);
    assert_eq!(leaves.first().unwrap().2, b"plain");
    assert_eq!(leaves.get(1).unwrap().2, b"<p>hidden alternative</p>");
    assert!(report.gaps.is_empty());
}

#[test]
fn malformed_transfer_and_exhausted_budget_are_visible_without_successful_leaves() {
    for input in [b"Content-Transfer-Encoding: base64\r\n\r\na!b".as_slice(), b"Content-Transfer-Encoding: quoted-printable\r\n\r\n=XY".as_slice()] {
        let report = visit(input, 1000, |_| panic!("malformed content must not be presented as complete")).unwrap();
        assert_eq!(report.leaves, 0);
        assert_eq!(report.gaps.first().unwrap().code, "malformed");
    }
    let report = visit(b"\r\nlong text", 2, |_| panic!("the decoded allowance must be enforced")).unwrap();
    assert_eq!(report.gaps.first().unwrap().code, "resourceLimit");
}
