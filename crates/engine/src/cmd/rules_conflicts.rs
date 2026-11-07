// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Read-only analysis shared by the folder editor and MCP, across destinations.

use mailcraft_model::{
    Mailbox, Rule,
    rule_conflicts::{competing_moves, move_destinations, proven_overlap},
};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, Session, p};

fn shares_account(left: &Rule, right: &Rule) -> bool {
    left.account == 0 || right.account == 0 || left.account == right.account
}

fn candidates<'a>(mailbox: &'a Mailbox, rule: &'a Rule) -> impl Iterator<Item = Rule> + 'a {
    mailbox
        .rules
        .iter()
        .filter(move |other| {
            other.id != rule.id
                && (other.enabled || other.server.as_ref().is_some_and(|server| !server.id.is_empty() && server.synced_enabled))
                && other.known()
                && shares_account(rule, other)
                && competing_moves(rule, other)
        })
        .cloned()
        .map(|mut other| {
            other.enabled = true;
            other
        })
}

pub(super) fn ensure_unambiguous(s: &Session, rule: &Rule) -> Result<(), CmdError> {
    if !rule.enabled {
        return Ok(());
    }
    if move_destinations(rule).len() > 1 {
        return Err(fail("one rule cannot move the same message to competing destinations; use explicit copies if intended"));
    }
    if let Some(other) = candidates(&s.mailbox, rule).find(|other| proven_overlap(&s.mailbox, rule, other)) {
        return Err(fail(format!(
            "filing conflict with \"{}\" (#{}), destinations {:?}; narrow the conditions or disable one rule before enabling both",
            other.name,
            other.id,
            move_destinations(&other)
        )));
    }
    Ok(())
}

fn destinations(mailbox: &Mailbox, rule: &Rule) -> Vec<Value> {
    move_destinations(rule)
        .into_iter()
        .map(|id| {
            let mut parts = Vec::new();
            let mut next = Some(id);
            // A malformed parent cycle cannot make diagnostics loop forever.
            for _ in 0..mailbox.folders.len() {
                let Some(folder) = next.and_then(|id| mailbox.folder(id)) else { break };
                parts.push(folder.name.clone());
                next = folder.parent;
            }
            parts.reverse();
            let account = mailbox.folder(id).map_or(0, |folder| folder.account);
            json!({"id": id, "account": account, "path": parts.join(" / "),
            "email": mailbox.account(account).map(|account| account.email.as_str())})
        })
        .collect()
}

pub fn inspect(s: &mut Session, params: &Value) -> CmdResult {
    let id = p::id_strict(params, "id")?;
    let mut rule = match params.get("rule") {
        Some(draft) => super::rules::rule_from_params(s, draft)?,
        None => s.mailbox.rules.iter().find(|rule| Some(rule.id) == id).cloned().ok_or_else(|| bad("provide a stored rule id or a draft rule"))?,
    };
    if let Some(id) = id {
        if !s.mailbox.rules.iter().any(|rule| rule.id == id) {
            return Err(bad("no rule with that id"));
        }
        rule.id = id;
    }
    // Analyze the enabled form even for a disabled draft; inspection never enables it.
    rule.enabled = true;
    let conflicts: Vec<_> = candidates(&s.mailbox, &rule)
        .map(|other| {
            let confirmed = proven_overlap(&s.mailbox, &rule, &other);
            json!({"id": other.id, "name": other.name, "account": other.account, "stop": other.stop,
            "certainty": if confirmed {"confirmed"} else {"potential"}, "destinations": destinations(&s.mailbox, &other),
            "serverEnabled":other.server.as_ref().map(|server|server.synced_enabled),
            "text": other.describe_as(s.mailbox.rule_reads_addresses(&other))})
        })
        .collect();
    let confirmed = conflicts.iter().filter(|conflict| conflict["certainty"] == "confirmed").count();
    let unknown = s.mailbox.rules.iter().filter(|other| other.enabled && !other.known() && shares_account(&rule, other)).count();
    Ok(json!({"conflicts": conflicts, "confirmed": confirmed, "potential": conflicts.len() - confirmed,
        "destinations": destinations(&s.mailbox, &rule), "unknownServerRules": unknown,
        "competingActions": move_destinations(&rule).len() > 1,
        "coverage": "bounded constructive analysis; unproved overlaps remain potential, not conflict-free",
        "mutated": false}))
}
