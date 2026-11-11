// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SimpleLogin's reported forwarding identities, separate from transport and reply routing.
//!
//! Retained message headers are claims, not a new authentication source. In particular a
//! passing DKIM result for a reverse alias does not authenticate arbitrary custom headers.
//!
//! Any sender can write `X-SimpleLogin-*` headers, so a claim is only *corroborated* (and only
//! then gives an alias folder, alias colour or reported original sender) when the claimed alias
//! is one of the message's own recipients and the message shows a SimpleLogin relay hop or
//! signature. This is a plausibility check against casual forgery, not authentication.

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
    /// The alias is a recipient and a SimpleLogin relay appears in the trace or signatures.
    pub corroborated: bool,
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

/// Domains SimpleLogin relays and signs forwarded mail from.
const RELAYS: [&str; 2] = ["simplelogin.co", "simplelogin.com"];

fn relay_domain(word: &str) -> bool {
    let host = word.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-')).trim_end_matches('.').to_ascii_lowercase();
    RELAYS.iter().any(|relay| host == *relay || host.strip_suffix(relay).is_some_and(|head| head.ends_with('.')))
}

/// A `Received` hop from a SimpleLogin host, a SimpleLogin `DKIM-Signature`, or a passing DKIM
/// result for a SimpleLogin signer in an (ARC-)Authentication-Results header.
fn relayed(message: &Message) -> bool {
    message.headers.iter().take(crate::auth_results::MAX_HEADERS_READ).any(|(name, value)| {
        let value: &str = value;
        let value = value.get(..value.len().min(4096)).unwrap_or(value);
        if name.eq_ignore_ascii_case("Received") {
            let mut words = value.split(|c: char| c.is_whitespace() || "()[];<>".contains(c)).filter(|word| !word.is_empty());
            return words.next().is_some_and(|first| first.eq_ignore_ascii_case("from"))
                && words.take_while(|word| !word.eq_ignore_ascii_case("by")).any(relay_domain);
        }
        if name.eq_ignore_ascii_case("DKIM-Signature") {
            return value.split(';').any(|tag| {
                let tag = tag.trim();
                tag.get(..2).is_some_and(|key| key.eq_ignore_ascii_case("d=")) && tag.get(2..).is_some_and(relay_domain)
            });
        }
        if name.eq_ignore_ascii_case("Authentication-Results") || name.eq_ignore_ascii_case("ARC-Authentication-Results") {
            return value.split(';').any(|clause| {
                let mut words = clause.split_whitespace();
                words.next().is_some_and(|result| result.eq_ignore_ascii_case("dkim=pass"))
                    && words
                        .any(|word| word.split_once('=').is_some_and(|(key, domain)| key.eq_ignore_ascii_case("header.d") && relay_domain(domain)))
            });
        }
        false
    })
}

/// Whether `alias` is among the message's To/Cc/Bcc recipients or delivery headers.
fn addressed_to(message: &Message, alias: &str) -> bool {
    message.to.iter().chain(&message.cc).chain(&message.bcc).any(|recipient| recipient.email.trim().eq_ignore_ascii_case(alias))
        || message.headers.iter().take(crate::auth_results::MAX_HEADERS_READ).any(|(name, value)| {
            ["Delivered-To", "X-Original-To", "Envelope-To"].iter().any(|wanted| name.eq_ignore_ascii_case(wanted))
                && value.len() <= 4096
                && Address::parse_list(value).iter().any(|recipient| recipient.email.trim().eq_ignore_ascii_case(alias))
        })
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
        let addressed = alias.as_deref().is_some_and(|alias| addressed_to(self, alias));
        if alias.is_some() && !addressed {
            issues.push(Issue { field: "receivingAlias", reason: "notRecipient" });
        }
        let relay = relayed(self);
        if !relay {
            issues.push(Issue { field: "relay", reason: "missing" });
        }
        let corroborated = forward && addressed && relay;
        Some(Identity {
            provenance: "messageHeaders",
            authenticated: false,
            corroborated,
            receiving_alias: alias.filter(|_| corroborated),
            original_from: original_from.filter(|_| corroborated),
            envelope_from: envelope_from.filter(|_| corroborated),
            issues,
        })
    }
}
