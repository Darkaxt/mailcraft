// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Filing ambiguity is checked against the original message, before any action.

use crate::{Action, Address, Condition, Id, Mailbox, Message, Rule, RuleField};
use std::collections::HashSet;

fn basic_senders(rule: &Rule) -> Vec<&str> {
    match rule.conditions.as_slice() {
        [Condition::FromContains(sender)] => vec![sender.as_str()],
        [Condition::FromAny(senders) | Condition::FromAddresses(senders)] => senders.iter().map(String::as_str).collect(),
        _ => Vec::new(),
    }
}

/// Only competing moves conflict. Multiple explicit copies are intentional.
/// These are the literal folder targets, for display; conflicts use [`filing_destinations`].
pub fn move_destinations(rule: &Rule) -> Vec<Id> {
    let mut destinations = Vec::new();
    for action in &rule.actions {
        if let Action::MoveTo(folder) = action
            && !destinations.contains(folder)
        {
            destinations.push(*folder);
        }
    }
    destinations
}

/// The one destination standing for removal: Delete and any move into a Deleted Items
/// subtree. Never an allocated folder id.
pub const DELETED_DESTINATION: Id = 0;

/// Where a rule's moves leave the message, with deletion as one competing destination.
pub fn filing_destinations(mailbox: &Mailbox, rule: &Rule) -> Vec<Id> {
    let mut destinations = Vec::new();
    for action in &rule.actions {
        let destination = match action {
            Action::MoveTo(folder) if mailbox.in_deleted_subtree(*folder) => DELETED_DESTINATION,
            Action::MoveTo(folder) => *folder,
            Action::Delete => DELETED_DESTINATION,
            _ => continue,
        };
        if !destinations.contains(&destination) {
            destinations.push(destination);
        }
    }
    destinations
}

pub fn competing_moves(mailbox: &Mailbox, left: &Rule, right: &Rule) -> bool {
    let a = filing_destinations(mailbox, left);
    let b = filing_destinations(mailbox, right);
    a.iter().any(|a| b.iter().any(|b| a != b))
}

/// A bounded constructive proof, never a claim that unproved regex pairs are disjoint.
/// Candidate strings come from literal conditions; the actual matchers verify the witness,
/// including exceptions, case behavior and address-only semantics. Unexamined combinations
/// remain potential overlaps. No email or native action is created by this analysis.
pub fn proven_overlap(mailbox: &Mailbox, left: &Rule, right: &Rule) -> bool {
    let mut message = Message::default();
    // Basic sender groups get complete exact-source intersection, including their last
    // alternative. The bounded constructive search below is for more complex definitions.
    let right_senders: HashSet<String> = basic_senders(right).iter().map(|sender| sender.to_lowercase()).collect();
    for sender in basic_senders(left).into_iter().filter(|sender| right_senders.contains(&sender.to_lowercase())) {
        message.from = Address::new("", sender);
        if left.matches_as(&message, mailbox.rule_reads_addresses(left)) && right.matches_as(&message, mailbox.rule_reads_addresses(right)) {
            return true;
        }
    }
    let mut senders = vec![String::new()];
    let mut aliases = vec!["witness@example.org".to_string()];
    let mut originals = vec![Address::new("", "witness@example.org")];
    for condition in left.conditions.iter().chain(&right.conditions) {
        match condition {
            Condition::FromContains(text) => senders.push(text.clone()),
            Condition::FromAny(values) | Condition::FromAddresses(values) => senders.extend(values.iter().take(32).cloned()),
            Condition::ToContains(text) => message.to.push(Address::new("", text)),
            Condition::ToAddresses(values) => message.to.extend(values.iter().map(|value| Address::new("", value))),
            Condition::SubjectContains(text) => append(&mut message.subject, text),
            Condition::BodyContains(text) => append(&mut message.body.text, text),
            Condition::HasAttachment => message.attachments.push(crate::Attachment::default()),
            Condition::Importance(importance) => message.importance = *importance,
            Condition::Text(text) if !text.regex => match text.field {
                RuleField::From => senders.push(text.pattern.clone()),
                RuleField::To => message.to.push(Address::new("", &text.pattern)),
                RuleField::Subject => append(&mut message.subject, &text.pattern),
                RuleField::Body => append(&mut message.body.text, &text.pattern),
                RuleField::ReceivingAlias => {
                    if crate::search::is_address(&text.pattern) {
                        aliases.push(text.pattern.clone());
                    }
                }
                RuleField::OriginalFrom => originals.push(if crate::search::is_address(&text.pattern) {
                    Address::new("", &text.pattern)
                } else {
                    Address::new(&text.pattern, "witness@example.org")
                }),
            },
            _ => {}
        }
    }
    let mut examined = 0;
    for alias in &aliases {
        for original in &originals {
            message.headers = vec![
                ("X-SimpleLogin-Type".into(), "Forward".into()),
                ("X-SimpleLogin-Envelope-To".into(), alias.clone()),
                ("X-SimpleLogin-Original-From".into(), original.full()),
                // The witness is relayed to the alias, as real SimpleLogin mail is (see simple_login).
                ("Delivered-To".into(), alias.clone()),
                ("Received".into(), "from relay.simplelogin.co by witness.example.org".into()),
            ];
            for sender in &senders {
                if examined >= 64 {
                    return false;
                }
                examined += 1;
                message.from = Address::new("", sender);
                if left.matches_as(&message, mailbox.rule_reads_addresses(left)) && right.matches_as(&message, mailbox.rule_reads_addresses(right)) {
                    return true;
                }
            }
        }
    }
    false
}

fn append(value: &mut String, text: &str) {
    if !value.is_empty() {
        value.push('\n');
    }
    value.push_str(text);
}

impl Mailbox {
    /// Deleted Items itself or any folder below it, by parent identity within its account.
    pub fn in_deleted_subtree(&self, folder: Id) -> bool {
        let mut next = self.folder(folder);
        for _ in 0..self.folders.len() {
            let Some(current) = next else { return false };
            if current.kind == crate::FolderKind::Deleted {
                return true;
            }
            next = current.parent.and_then(|parent| self.folder(parent)).filter(|parent| parent.account == current.account);
        }
        false
    }

    /// A per-message filing exception never broadens a sender's standing rule.
    pub fn filing_rule_overridden(&self, message: &Message, rule: &Rule) -> bool {
        message
            .filing_override
            .is_some_and(|destination| rule.actions.iter().any(|action| matches!(action, Action::MoveTo(folder) if *folder != destination)))
    }

    /// Inspect original and sequential importance states independently of Stop. A priority
    /// winner must not hide two definitions claiming different destinations.
    pub fn conflicting_filing_rules(&self, message: Id, now: bool) -> Vec<&Rule> {
        let Some(message) = self.message(message) else { return Vec::new() };
        let mut matching = Vec::new();
        let mut importance = message.importance;
        for index in self.rules_in_order(message.account) {
            let Some(rule) = self.rules.get(index) else { continue };
            if self.filing_rule_overridden(message, rule) {
                continue;
            }
            if self.validate_rule_actions(&rule.actions).is_err() {
                // The normal evaluator reports and pauses this incomplete definition.
                continue;
            }
            let enabled = rule.server.as_ref().filter(|server| !now && !server.id.is_empty()).map_or(rule.enabled, |server| server.synced_enabled);
            let normalized = if enabled == rule.enabled {
                None
            } else {
                let mut normalized = rule.clone();
                normalized.enabled = enabled;
                Some(normalized)
            };
            let effective = normalized.as_ref().unwrap_or(rule);
            let addresses = self.rule_reads_addresses(rule);
            let in_sequence = effective.matches_at_importance(message, addresses, importance);
            if !filing_destinations(self, rule).is_empty() && (in_sequence || effective.matches_as(message, addresses)) {
                matching.push(rule);
            }
            if in_sequence && (now || !rule.on_server()) {
                for action in &rule.actions {
                    if let Action::SetImportance(next) = action {
                        importance = *next;
                    }
                }
            }
        }
        // Two folder moves inside one rule compete; one rule's own move-then-delete is a
        // sequence, but deletion competes with any other rule's filing destination.
        let competing = matching.iter().any(|rule| move_destinations(rule).len() > 1)
            || matching.iter().enumerate().any(|(index, left)| matching.iter().skip(index + 1).any(|right| competing_moves(self, left, right)));
        if competing { matching } else { Vec::new() }
    }
}
