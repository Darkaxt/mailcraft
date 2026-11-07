// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit local text matching. Compiled expressions have bounded shared retention.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::{Message, contains_ci};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleField {
    From,
    To,
    Subject,
    Body,
    OriginalFrom,
    ReceivingAlias,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuleText {
    pub field: RuleField,
    pub pattern: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
}

type Cached = (String, bool, Result<Regex, String>);
// Retain at most sixteen bounded regex programs, never one for every stored rule.
// The key includes the complete expression and case setting. Matching happens
// outside the lock; Regex clones share compiled storage safely.
static CACHE: OnceLock<Mutex<VecDeque<Cached>>> = OnceLock::new();

fn compiled(pattern: &str, case_sensitive: bool) -> Result<Regex, String> {
    if pattern.is_empty() || pattern.len() > 4096 {
        return Err("a rule expression must contain 1 to 4096 UTF-8 bytes".into());
    }
    let cache = CACHE.get_or_init(|| Mutex::new(VecDeque::new()));
    let mut cached = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(index) = cached.iter().position(|(p, case, _)| p == pattern && *case == case_sensitive)
        && let Some(entry) = cached.remove(index)
    {
        let result = entry.2.clone();
        cached.push_back(entry);
        return result;
    }
    let result = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .size_limit(256 * 1024)
        .dfa_size_limit(64 * 1024)
        .nest_limit(64)
        .build()
        .map_err(|error| format!("invalid or unsupported rule expression: {error}"));
    if cached.len() >= 16 {
        cached.pop_front();
    }
    cached.push_back((pattern.to_string(), case_sensitive, result.clone()));
    result
}

impl RuleText {
    pub fn validate(&self) -> Result<(), String> {
        if self.pattern.is_empty() || self.pattern.len() > 4096 {
            return Err("a rule expression must contain 1 to 4096 UTF-8 bytes".into());
        }
        if self.regex {
            compiled(&self.pattern, self.case_sensitive)?;
        }
        Ok(())
    }

    pub fn matches(&self, message: &Message, addresses_only: bool) -> bool {
        if self.validate().is_err() {
            return false;
        }
        let regex = if self.regex { compiled(&self.pattern, self.case_sensitive).ok() } else { None };
        let text = |value: &str| match &regex {
            Some(regex) => regex.is_match(value),
            None if self.regex => false,
            None if self.case_sensitive => value.contains(&self.pattern),
            None => contains_ci(value, &self.pattern),
        };
        let person = |address: &crate::Address| text(&address.email) || (!addresses_only && text(&address.name));
        match self.field {
            RuleField::From => person(&message.from),
            RuleField::To => message.to.iter().chain(&message.cc).any(person),
            RuleField::Subject => text(&message.subject),
            RuleField::Body => text(&message.body.text) || text(&message.hidden_text),
            RuleField::OriginalFrom => message.simple_login().and_then(|identity| identity.original_from).is_some_and(|original| person(&original)),
            RuleField::ReceivingAlias => message.simple_login().and_then(|identity| identity.receiving_alias).is_some_and(|alias| text(&alias)),
        }
    }

    pub fn describe(&self) -> String {
        let field = match self.field {
            RuleField::From => "from",
            RuleField::To => "recipient",
            RuleField::Subject => "subject",
            RuleField::Body => "body",
            RuleField::OriginalFrom => "SimpleLogin reported sender",
            RuleField::ReceivingAlias => "SimpleLogin receiving alias",
        };
        let mode = if self.regex { "matches regex" } else { "contains" };
        let case = if self.case_sensitive { "case sensitive" } else { "case insensitive" };
        format!("{field} {mode} \"{}\" ({case})", self.pattern)
    }
}
