// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account's colour (#893): kept in the mailbox file, read from a file from before there was
//! one, and given to a new account from the palette.

use serde_json::json;

use crate::*;

/// A colour is written with its account and read back as it was; none is not written at all.
#[test]
fn an_accounts_colour_is_kept_in_the_file() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let work = mb.add_account(Account { email: "ada@work.example".into(), color: Some(Rgb(0x2E, 0x8B, 0x57)), ..Default::default() });
    let text = serde_json::to_string(&mb).unwrap();
    let back: Mailbox = serde_json::from_str(&text).unwrap();
    assert_eq!(back.account(work).unwrap().color, Some(Rgb(0x2E, 0x8B, 0x57)));
    assert_eq!(back.accounts[0].color, None);
    let first = serde_json::to_value(&mb.accounts[0]).unwrap();
    assert!(first.get("color").is_none(), "{first}");
}

/// A file from before there were colours has none; one that says something this version can't
/// read as a colour loses the colour and nothing else.
#[test]
fn a_file_without_colours_or_with_a_bad_one_still_opens() {
    for color in [None, Some(json!("red")), Some(json!([999, 0])), Some(json!({"r": 1})), Some(json!(null))] {
        let mut account = json!({"id": 7, "name": "Ada", "email": "ada@example.org", "displayName": "Ada Lovelace"});
        if let Some(color) = &color {
            account["color"] = color.clone();
        }
        let read: Account = serde_json::from_value(account).unwrap();
        assert_eq!(read.color, None, "{color:?}");
        assert_eq!(read.email, "ada@example.org");
    }
    let read: Account = serde_json::from_value(json!({"id": 7, "email": "ada@example.org", "color": [26, 155, 166]})).unwrap();
    assert_eq!(read.color, Some(Rgb(0x1A, 0x9B, 0xA6)));
}

/// A new account is given the first colour of the palette no account has, and when all are
/// taken, the one the fewest have.
#[test]
fn a_new_account_gets_a_colour_no_other_has() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    assert_eq!(mb.unused_account_color(), PALETTE[0]);
    mb.accounts[0].color = Some(PALETTE[0]);
    assert_eq!(mb.unused_account_color(), PALETTE[1]);
    // (A colour of the owner's own choosing, off the palette, takes nothing from it.)
    mb.add_account(Account { email: "b@example.org".into(), color: Some(Rgb(1, 2, 3)), ..Default::default() });
    assert_eq!(mb.unused_account_color(), PALETTE[1]);
    for (n, c) in PALETTE.iter().enumerate().skip(1) {
        mb.add_account(Account { email: format!("{n}@example.org"), color: Some(*c), ..Default::default() });
    }
    assert_eq!(mb.unused_account_color(), PALETTE[0]);
    mb.add_account(Account { email: "x@example.org".into(), color: Some(PALETTE[0]), ..Default::default() });
    assert_eq!(mb.unused_account_color(), PALETTE[1]);
}

/// A row's origin colour is the account's own (unlightened) unless one of its own was chosen; an
/// alias's own colour wins over both, and an account with neither keeps the palette colour of its
/// stable id.
#[test]
fn rows_follow_the_accounts_colour_until_one_is_chosen() {
    let green = Rgb(0x2E, 0x8B, 0x57);
    let mut account = Account { id: 3, color: Some(green), ..Default::default() };
    assert_eq!(account.effective_row_color(), green);
    let chosen = Rgb(232, 212, 250);
    account.row_color = Some(chosen);
    assert_eq!(account.effective_row_color(), chosen);
    account.alias_row_colors.insert("shop@example.org".into(), Rgb(213, 238, 220));
    assert_eq!(account.origin_row_color(Some("Shop@Example.org")), Rgb(213, 238, 220));
    assert_eq!(account.origin_row_color(Some("other@example.org")), chosen);
    let plain = Account { id: 3, ..Default::default() };
    assert_eq!(plain.effective_row_color(), ACCOUNT_ROW_COLORS[3]);
}

/// The default origin palette is saturated and distinct: no near-white pastel, no two alike.
#[test]
fn default_origin_palette_is_saturated_and_distinct() {
    for (i, c) in ACCOUNT_ROW_COLORS.iter().enumerate() {
        let (max, min) = (c.0.max(c.1).max(c.2), c.0.min(c.1).min(c.2));
        assert!(max - min >= 80, "{} is not saturated", c.hex());
        assert!(min < 0xB0, "{} is too pale", c.hex());
        for other in &ACCOUNT_ROW_COLORS[i + 1..] {
            let d = (i32::from(c.0) - i32::from(other.0)).abs()
                + (i32::from(c.1) - i32::from(other.1)).abs()
                + (i32::from(c.2) - i32::from(other.2)).abs();
            assert!(d >= 90, "{} and {} are too alike", c.hex(), other.hex());
        }
    }
}
