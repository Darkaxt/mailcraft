// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Consolidate equivalent local filing rules while preserving other work.

use mailcraft_model::{Action, Condition, Id, Keep, Rule, RuleMergeSource};

use crate::Session;

fn senders(rule: &Rule) -> Option<(Vec<String>, bool)> {
    if rule.keep != Keep::Here || rule.server.is_some() || rule.unanswered || !rule.refused.is_empty() || !rule.repair.is_empty() {
        return None;
    }
    if !matches!(rule.actions.as_slice(), [Action::MoveTo(_)]) || rule.validation().is_err() {
        return None;
    }
    match rule.conditions.as_slice() {
        [Condition::FromContains(sender)] => Some((vec![sender.clone()], false)),
        [Condition::FromAny(senders)] => Some((senders.clone(), false)),
        [Condition::FromAddresses(senders)] => Some((senders.clone(), true)),
        _ => None,
    }
}

fn originals(rule: &Rule, senders: &[String], exact: bool) -> Vec<RuleMergeSource> {
    if !rule.merged_from.is_empty() {
        return rule.merged_from.clone();
    }
    let destination = match rule.actions.first() {
        Some(Action::MoveTo(folder)) => *folder,
        _ => 0,
    };
    vec![RuleMergeSource {
        id: rule.id,
        name: rule.name.clone(),
        account: rule.account,
        enabled: rule.enabled,
        stop: rule.stop,
        destination,
        senders: senders.to_vec(),
        sender_addresses: exact,
    }]
}

/// Moving the later sender alternatives earlier must not bypass another effect.
/// Scope alone proves independent accounts; within a shared scope only a known
/// positive exact-address predicate can prove the alternatives never match.
fn can_cross(rule: &Rule, account: Id, moving: &[String], exact: bool) -> bool {
    if rule.account != 0 && account != 0 && rule.account != account {
        return true;
    }
    exact
        && rule.keep == Keep::Here
        && rule.server.is_none()
        && !rule.unanswered
        && rule.refused.is_empty()
        && rule.repair.is_empty()
        && rule.validation().is_ok()
        && rule.conditions.iter().any(|condition| match condition {
            Condition::FromAddresses(addresses) => addresses.iter().all(|address| moving.iter().all(|sender| !sender.eq_ignore_ascii_case(address))),
            _ => false,
        })
}

/// Return the surviving identity and retired identities. Only adjacent rules
/// or alternatives proven independent of every intervening rule are combined.
pub(super) fn consolidate(s: &mut Session, mut id: Id) -> (Id, Vec<Id>) {
    let mut retired = Vec::new();
    while let Some(index) = s.mailbox.rules.iter().position(|rule| rule.id == id) {
        let pair = (0..index).rev().map(|before| (before, index)).chain((index + 1..s.mailbox.rules.len()).map(|after| (index, after))).find_map(
            |(left, right)| {
                let a = s.mailbox.rules.get(left)?;
                let b = s.mailbox.rules.get(right)?;
                if a.account != b.account
                    || a.enabled != b.enabled
                    || a.stop != b.stop
                    || a.actions != b.actions
                    || crate::agent::rule_can_delete_mail(&s.mailbox, a)
                    || crate::agent::rule_can_delete_mail(&s.mailbox, b)
                {
                    return None;
                }
                let ((mut sources, exact), (additions, other_exact)) = (senders(a)?, senders(b)?);
                if exact != other_exact {
                    return None;
                }
                if !s.mailbox.rules.get(left + 1..right)?.iter().all(|rule| can_cross(rule, a.account, &additions, exact)) {
                    return None;
                }
                let mut provenance = originals(a, &sources, exact);
                provenance.extend(originals(b, &additions, exact));
                for sender in additions {
                    if !sources.contains(&sender) {
                        sources.push(sender);
                    }
                }
                if sources.len() > mailcraft_model::MAX_RULE_SENDERS || provenance.len() > mailcraft_model::MAX_RULE_SENDERS {
                    return None; // Keep both exact definitions; never truncate a source.
                }
                let combined = if exact { Condition::FromAddresses(sources) } else { Condition::FromAny(sources) };
                if combined.validate().is_err() {
                    return None;
                }
                Some((left, right, combined, provenance))
            },
        );
        let Some((left, right, combined, provenance)) = pair else { break };
        let removed = s.mailbox.rules.remove(right);
        if let Some(survivor) = s.mailbox.rules.get_mut(left) {
            survivor.conditions = vec![combined];
            survivor.merged_from = provenance;
            id = survivor.id;
            retired.push(removed.id);
        }
    }
    (id, retired)
}
