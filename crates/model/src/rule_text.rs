// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Explicit local text matching. Compiled expressions have bounded shared retention.

use std::collections::HashMap;
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

/// Retained compiled programs. Bounded, never one per stored rule beyond this.
const CAPACITY: usize = 256;

/// A compiled (or refused) expression and when it was last used, for least-recent eviction.
struct Cached {
    result: Result<Regex, String>,
    used: u64,
}

/// Keyed by the complete expression, separately per case setting (index: case sensitive).
/// Compilation and matching happen outside the lock; Regex clones share compiled storage.
#[derive(Default)]
struct Cache {
    entries: [HashMap<String, Cached>; 2],
    clock: u64,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    CACHE.get_or_init(|| Mutex::new(Cache::default())).lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn sized(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() || pattern.len() > 4096 {
        return Err("a rule expression must contain 1 to 4096 UTF-8 bytes".into());
    }
    Ok(())
}

fn compiled(pattern: &str, case_sensitive: bool) -> Result<Regex, String> {
    sized(pattern)?;
    let slot = usize::from(case_sensitive);
    {
        let mut cache = cache();
        cache.clock += 1;
        let now = cache.clock;
        if let Some(entry) = cache.entries[slot].get_mut(pattern) {
            entry.used = now;
            return entry.result.clone();
        }
    }
    // Compile without holding the shared lock; a concurrent duplicate compile is harmless.
    let result = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .size_limit(256 * 1024)
        .dfa_size_limit(64 * 1024)
        .nest_limit(64)
        .build()
        .map_err(|error| format!("invalid or unsupported rule expression: {error}"));
    let mut cache = cache();
    cache.clock += 1;
    let used = cache.clock;
    if !cache.entries[slot].contains_key(pattern) && cache.entries.iter().map(HashMap::len).sum::<usize>() >= CAPACITY {
        let oldest = cache
            .entries
            .iter()
            .enumerate()
            .flat_map(|(slot, entries)| entries.iter().map(move |(pattern, entry)| (entry.used, slot, pattern)))
            .min_by_key(|(used, _, _)| *used)
            .map(|(_, slot, pattern)| (slot, pattern.clone()));
        if let Some((slot, pattern)) = oldest {
            cache.entries[slot].remove(&pattern);
        }
    }
    cache.entries[slot].insert(pattern.to_string(), Cached { result: result.clone(), used });
    result
}

impl RuleText {
    pub fn validate(&self) -> Result<(), String> {
        sized(&self.pattern)?;
        if self.regex {
            compiled(&self.pattern, self.case_sensitive)?;
        }
        Ok(())
    }

    pub fn matches(&self, message: &Message, addresses_only: bool) -> bool {
        // One size check and at most one cache lookup per match, not validate-then-compile.
        if sized(&self.pattern).is_err() {
            return false;
        }
        let regex = if self.regex {
            match compiled(&self.pattern, self.case_sensitive) {
                Ok(regex) => Some(regex),
                Err(_) => return false,
            }
        } else {
            None
        };
        let text = |value: &str| match &regex {
            Some(regex) => regex.is_match(value),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_distinct_expressions_stay_cached_and_bounded() {
        let patterns: Vec<String> = (0..64).map(|n| format!("^cache-probe-{n}-[0-9]+$")).collect();
        for _ in 0..2 {
            for (n, pattern) in patterns.iter().enumerate() {
                let message = Message { subject: format!("cache-probe-{n}-42"), ..Default::default() };
                let text = RuleText { field: RuleField::Subject, pattern: pattern.clone(), regex: true, case_sensitive: true };
                assert!(text.matches(&message, false), "{pattern}");
            }
        }
        // More than sixteen expressions remain retained instead of thrashing.
        let cache = cache();
        assert!(patterns.iter().all(|pattern| cache.entries[1].contains_key(pattern)));
        assert!(cache.entries.iter().map(HashMap::len).sum::<usize>() <= CAPACITY);
    }

    #[test]
    fn invalid_and_oversized_expressions_never_match() {
        let message = Message { subject: "x".into(), ..Default::default() };
        for pattern in ["(".to_string(), String::new(), "x".repeat(4097)] {
            let text = RuleText { field: RuleField::Subject, pattern, regex: true, case_sensitive: false };
            assert!(text.validate().is_err());
            assert!(!text.matches(&message, false));
        }
    }
}
