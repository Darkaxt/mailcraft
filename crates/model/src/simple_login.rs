// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SimpleLogin's reported forwarding identities, separate from transport and reply routing.
//!
//! Retained message headers are claims, not a new authentication source. In particular a
//! passing DKIM result for a reverse alias does not authenticate arbitrary custom headers.

use serde::Serialize;

use crate::{Address, Message};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub field: &'static str,
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub provenance: &'static str,
    pub authenticated: bool,
    pub receiving_alias: Option<String>,
    pub original_from: Option<Address>,
    pub envelope_from: Option<String>,
    pub issues: Vec<Issue>,
}

fn field<'a>(message: &'a Message, header: &str, name: &'static str, required: bool, issues: &mut Vec<Issue>) -> Option<&'a str> {
    let mut values = message.headers.iter().filter(|(key, _)| key.eq_ignore_ascii_case(header));
    let first = values.next();
    if values.next().is_some() {
        issues.push(Issue { field: name, reason: "duplicate" });
        return None;
    }
    let Some((_, value)) = first else {
        if required {
            issues.push(Issue { field: name, reason: "missing" });
        }
        return None;
    };
    if value.len() > 4096 || value.chars().any(|character| character.is_control() || mailcraft_names::is_format(character)) {
        issues.push(Issue { field: name, reason: "invalid" });
        return None;
    }
    Some(value.trim())
}

fn address(value: &str) -> Option<Address> {
    // Address::parse accepts a useful editing shorthand, including a suffix after `>`.
    // A reported identity instead has to contain exactly one whole mailbox.
    if value.rfind('>').is_some_and(|end| value.get(end.saturating_add(1)..).is_none_or(|tail| !tail.trim().is_empty())) {
        return None;
    }
    let mut addresses = Address::parse_list(value).into_iter();
    let one = addresses.next()?;
    (addresses.next().is_none() && one.email.len() <= crate::MAX_ADDRESS && crate::search::is_address(&one.email)).then_some(one)
}

impl Message {
    /// Header-reported SimpleLogin metadata. Never modifies `from`, `reply_to`, or recipients.
    pub fn simple_login(&self) -> Option<Identity> {
        if !self.headers.iter().any(|(key, _)| {
            key.eq_ignore_ascii_case("X-SimpleLogin-Type")
                || key.eq_ignore_ascii_case("X-SimpleLogin-Envelope-To")
                || key.eq_ignore_ascii_case("X-SimpleLogin-Original-From")
        }) {
            return None;
        }
        let mut issues = Vec::new();
        let direction = field(self, "X-SimpleLogin-Type", "direction", true, &mut issues);
        let forward = direction.is_some_and(|value| value.eq_ignore_ascii_case("Forward"));
        if direction.is_some() && !forward {
            issues.push(Issue { field: "direction", reason: "notForward" });
        }
        let alias = field(self, "X-SimpleLogin-Envelope-To", "receivingAlias", true, &mut issues).and_then(|value| {
            let found = address(value).filter(|address| address.name.is_empty());
            if found.is_none() {
                issues.push(Issue { field: "receivingAlias", reason: "invalid" });
            }
            found.map(|address| address.email)
        });
        let original_from = field(self, "X-SimpleLogin-Original-From", "originalFrom", true, &mut issues).and_then(|value| {
            let found = address(value);
            if found.is_none() {
                issues.push(Issue { field: "originalFrom", reason: "invalid" });
            }
            found
        });
        let envelope_from = field(self, "X-SimpleLogin-Envelope-From", "envelopeFrom", false, &mut issues).and_then(|value| {
            let found = address(value).filter(|address| address.name.is_empty());
            if found.is_none() {
                issues.push(Issue { field: "envelopeFrom", reason: "invalid" });
            }
            found.map(|address| address.email)
        });
        Some(Identity {
            provenance: "messageHeaders",
            authenticated: false,
            receiving_alias: alias.filter(|_| forward),
            original_from: original_from.filter(|_| forward),
            envelope_from: envelope_from.filter(|_| forward),
            issues,
        })
    }
}
