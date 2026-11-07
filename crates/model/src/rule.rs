// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail rules: conditions on an arriving message and what to do with it.
//!
//! **Whose rule (#566).** A rule is for the arriving mail of one account ([`Rule::account`]). A
//! rule from before rules had accounts has none: it is for every account's mail, and is kept on
//! this computer until the owner moves it to an account's server.
//!
//! **Where it is kept.** A rule of a Microsoft account can be kept on its server ([`Keep::Server`]),
//! which runs it whether MailCraft is open or not, and shows it in Outlook on the web and on the
//! phone. Once the server has it ([`Rule::server`]) it never runs here on arriving mail: the
//! server has run it already. Until then it runs here, as every rule kept on this computer does.
//! A rule made in Outlook or on the web is the server's too; one with parts this version doesn't
//! know ([`ServerRule::unknown`]) is switched on and off, moved and deleted, never rewritten and
//! never run here.

use serde::{Deserialize, Serialize};

use crate::{Id, Importance, Message, contains_ci};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Condition {
    /// The sender's name or address contains the text (a rule kept on a Microsoft account's
    /// server: the address alone, as the server reads it).
    FromContains(String),
    /// Local OR of sender substrings, retaining FromContains matching semantics.
    FromAny(Vec<String>),
    /// Exact transport sender addresses, independent of the display name.
    FromAddresses(Vec<String>),
    /// Exact To/Cc addresses, independent of display names.
    ToAddresses(Vec<String>),
    /// Any To/Cc recipient's name or address contains the text (a rule kept on a Microsoft
    /// account's server: the addresses alone).
    ToContains(String),
    SubjectContains(String),
    BodyContains(String),
    HasAttachment,
    Importance(Importance),
    /// Local literal or regular expression with explicit case behavior.
    Text(crate::RuleText),
    /// Any matching exception vetoes the rule. An explicit condition variant
    /// prevents older readers from ignoring exceptions as an unknown field.
    Unless(Vec<Condition>),
}

impl Condition {
    pub fn matches(&self, m: &Message) -> bool {
        self.matches_as(m, false)
    }

    /// [`Condition::matches`], with `addresses_only` for a rule of an account whose server keeps
    /// rules: its server looks for the words of a sender or a recipient in their addresses
    /// alone (Exchange's "words in the sender's address"), and so does this computer for it.
    pub fn matches_as(&self, m: &Message, addresses_only: bool) -> bool {
        let person = |a: &crate::Address, t: &str| contains_ci(&a.email, t) || (!addresses_only && contains_ci(&a.name, t));
        match self {
            Condition::FromContains(t) => person(&m.from, t),
            Condition::FromAny(senders) => self.validate().is_ok() && senders.iter().any(|sender| person(&m.from, sender)),
            Condition::FromAddresses(addresses) => {
                self.validate().is_ok() && addresses.iter().any(|address| m.from.email.eq_ignore_ascii_case(address))
            }
            Condition::ToAddresses(addresses) => {
                self.validate().is_ok()
                    && m.to.iter().chain(&m.cc).any(|recipient| addresses.iter().any(|address| recipient.email.eq_ignore_ascii_case(address)))
            }
            Condition::ToContains(t) => m.to.iter().chain(&m.cc).any(|a| person(a, t)),
            Condition::SubjectContains(t) => contains_ci(&m.subject, t),
            Condition::BodyContains(t) => contains_ci(&m.body.text, t) || contains_ci(&m.hidden_text, t),
            Condition::HasAttachment => m.has_attachments(),
            Condition::Importance(i) => m.importance == *i,
            Condition::Text(text) => text.matches(m, addresses_only),
            Condition::Unless(exceptions) => self.validate().is_ok() && !exceptions.iter().any(|c| c.matches_as(m, addresses_only)),
        }
    }

    /// The condition in words: `from contains "newsletter"`.
    pub fn describe(&self) -> String {
        self.describe_as(false)
    }

    /// [`Condition::describe`] for a rule that reads addresses alone ([`Condition::matches_as`]).
    pub fn describe_as(&self, addresses_only: bool) -> String {
        match self {
            Condition::FromContains(t) if addresses_only => format!("sender's address contains \"{t}\""),
            Condition::ToContains(t) if addresses_only => format!("recipient's address contains \"{t}\""),
            Condition::FromContains(t) => format!("from contains \"{t}\""),
            Condition::FromAny(senders) => {
                format!("from contains any of ({})", senders.iter().map(|sender| format!("\"{sender}\"")).collect::<Vec<_>>().join(", "))
            }
            Condition::FromAddresses(addresses) | Condition::ToAddresses(addresses) => {
                let field = if matches!(self, Condition::FromAddresses(_)) { "sender's address" } else { "a To/Cc address" };
                format!("{field} is one of ({})", addresses.iter().map(|address| format!("\"{address}\"")).collect::<Vec<_>>().join(", "))
            }
            Condition::ToContains(t) => format!("sent to \"{t}\""),
            Condition::SubjectContains(t) => format!("subject contains \"{t}\""),
            Condition::BodyContains(t) => format!("body contains \"{t}\""),
            Condition::HasAttachment => "has an attachment".into(),
            Condition::Importance(i) => format!("importance is {}", i.code()),
            Condition::Text(text) => text.describe(),
            Condition::Unless(exceptions) => {
                format!("not ({})", exceptions.iter().map(|c| c.describe_as(addresses_only)).collect::<Vec<_>>().join(" or "))
            }
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Condition::FromAddresses(addresses) | Condition::ToAddresses(addresses) => {
                if addresses.is_empty()
                    || addresses.len() > MAX_RULE_SENDERS
                    || addresses.iter().any(|address| {
                        address.len() > 254
                            || address.trim() != address
                            || address.contains(['<', '>', ',', ';', '\r', '\n'])
                            || address.contains(char::is_control)
                            || address.contains(char::is_whitespace)
                            || address.split_once('@').is_none_or(|(local, domain)| {
                                local.is_empty() || !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') || domain.contains('@')
                            })
                    })
                {
                    return Err("exact address alternatives require 1 to 200 plain email addresses of at most 254 UTF-8 bytes each".into());
                }
                Ok(())
            }
            Condition::FromAny(senders) => {
                if senders.is_empty()
                    || senders.len() > MAX_RULE_SENDERS
                    || senders.iter().any(|sender| sender.trim().is_empty() || sender.chars().count() > 200 || sender.contains(['\r', '\n']))
                {
                    return Err("sender alternatives require 1 to 200 nonempty single-line values of at most 200 characters each".into());
                }
                Ok(())
            }
            Condition::Text(text) => text.validate(),
            Condition::Unless(exceptions) => {
                if exceptions.is_empty() || exceptions.len() > 20 || exceptions.iter().any(|c| matches!(c, Condition::Unless(_))) {
                    return Err("exceptions require 1 to 20 flat conditions".into());
                }
                for condition in exceptions {
                    condition.validate()?;
                }
                Ok(())
            }
            Condition::FromContains(text) | Condition::ToContains(text) | Condition::SubjectContains(text) | Condition::BodyContains(text)
                if text.trim().is_empty() =>
            {
                Err("a condition needs text".into())
            }
            _ => Ok(()),
        }
    }

    pub fn reads_body(&self) -> bool {
        let body = |c: &Condition| matches!(c, Condition::BodyContains(_) | Condition::Text(crate::RuleText { field: crate::RuleField::Body, .. }));
        body(self) || matches!(self, Condition::Unless(exceptions) if exceptions.iter().any(body))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Action {
    /// Move to the folder with this id.
    MoveTo(Id),
    CopyTo(Id),
    Categorize(String),
    Flag,
    MarkRead,
    SetImportance(Importance),
    /// Move to Deleted Items.
    Delete,
}

impl Action {
    pub fn describe(&self) -> String {
        match self {
            Action::MoveTo(_) => "move it to a folder".into(),
            Action::CopyTo(_) => "copy it to a folder".into(),
            Action::Categorize(c) => format!("categorize it as \"{c}\""),
            Action::Flag => "flag it".into(),
            Action::MarkRead => "mark it as read".into(),
            Action::SetImportance(i) => format!("set importance to {}", i.code()),
            Action::Delete => "delete it".into(),
        }
    }
}

/// Native rule effects, including failed actions. Matching is not proof of filing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleExecution {
    pub conflicts: bool,
    pub matched: usize,
    pub applied: Vec<String>,
    pub failures: Vec<RuleExecutionFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleExecutionFailure {
    pub rule: Id,
    pub name: String,
    pub message: Id,
    pub completed_actions: usize,
    pub error: String,
}

impl RuleExecutionFailure {
    pub fn describe(&self) -> String {
        format!("Rule \"{}\" on message {} failed after {} actions: {}", self.name, self.message, self.completed_actions, self.error)
    }
}

/// Where the owner wants a rule kept (#566).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Keep {
    /// On this computer: it runs while MailCraft is open.
    #[default]
    Here,
    /// On its account's server, which runs it whether MailCraft is open or not. Until the server
    /// has it ([`Rule::server`]), it runs here.
    Server,
}

impl Keep {
    pub fn is_here(&self) -> bool {
        *self == Keep::Here
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A rule as its account's server has it (#566).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerRule {
    /// What the server calls it.
    pub id: String,
    /// Where it stands among the server's rules (Graph's `sequence`), as it is to be: a move here
    /// changes it before the server is told.
    pub sequence: i32,
    /// Whether it is switched on, and where it stood, as the server last had it. What was
    /// changed here since is the difference ([`Rule::changed_here`]), and only that is sent.
    pub synced_enabled: bool,
    pub synced_sequence: i32,
    /// The parts of it this version doesn't know, by the server's names (`forwardTo`,
    /// `exceptions`…). Such a rule is switched on and off, moved and deleted, and never
    /// rewritten or run here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
    /// The server doesn't let it be changed or deleted (Graph's `isReadOnly`).
    #[serde(skip_serializing_if = "is_false")]
    pub read_only: bool,
    /// The server says it is in error (Graph's `hasError`).
    #[serde(skip_serializing_if = "is_false")]
    pub error: bool,
}

/// A rule deleted here that its server is yet to be told about (#566).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuleGone {
    pub account: Id,
    /// What the server calls it.
    pub id: String,
    /// A rule deleted here while the server was making it, of which no answer came: the server
    /// may have it under an id nobody here knows, and `id` is empty. The next full read of the
    /// server's rules finds the one there that looks the same, which is then deleted there, or
    /// doesn't, and this is let go of. So is it when an answer comes after all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub made: Option<Box<Rule>>,
    /// A definitive provider refusal: retain the unresolved deletion, but never replay it
    /// automatically. A complete later listing can still confirm that the rule is gone.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub blocked: String,
}

/// Bounded alternatives/provenance for a consolidated basic local filing rule.
pub const MAX_RULE_SENDERS: usize = 200;

/// The original basic filing definition, kept when automatic consolidation retires its identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleMergeSource {
    pub id: Id,
    pub name: String,
    pub account: Id,
    pub enabled: bool,
    pub stop: bool,
    pub destination: Id,
    pub senders: Vec<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub sender_addresses: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rule {
    pub id: Id,
    pub name: String,
    pub enabled: bool,
    /// All must hold. A rule without conditions matches nothing (it would act on every message).
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Later rules don't run on a message this one matched.
    pub stop: bool,
    /// The account whose arriving mail it is for; 0 for a rule from before rules had accounts,
    /// which is for every account's mail and kept on this computer.
    #[serde(skip_serializing_if = "crate::is_zero_id")]
    pub account: Id,
    /// Where the owner wants it kept.
    #[serde(skip_serializing_if = "Keep::is_here")]
    pub keep: Keep,
    /// The server's copy: for a rule its account's server has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerRule>,
    /// Why the server didn't take it, when it didn't: it is kept here, and said so.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub refused: String,
    /// A local definition with a missing/unsafe destination is paused until
    /// repaired. Older readers still see enabled=false and cannot run it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub repair: String,
    /// Original basic definitions, not executable nested rules.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub merged_from: Vec<RuleMergeSource>,
    /// It went to the server to be made there and no answer came: the server may have it. It is
    /// not sent again until the server's rules have been read whole, which either have it (and
    /// it is taken as this one) or don't (and it is sent again).
    #[serde(skip_serializing_if = "is_false")]
    pub unanswered: bool,
}

impl Rule {
    pub fn matches(&self, m: &Message) -> bool {
        self.matches_as(m, false)
    }

    /// [`Rule::matches`], reading addresses alone when `addresses_only` ([`Condition::matches_as`]).
    /// A rule with parts this version doesn't know matches nothing here: what it would do is not
    /// known.
    pub fn matches_as(&self, m: &Message, addresses_only: bool) -> bool {
        self.matches_at_importance(m, addresses_only, m.importance)
    }

    /// Conflict analysis can evaluate an earlier importance action without copying bodies
    /// or changing the real message. The ordinary evaluator uses its actual importance.
    pub(crate) fn matches_at_importance(&self, m: &Message, addresses_only: bool, importance: Importance) -> bool {
        let ordinary = |condition: &Condition| match condition {
            Condition::Importance(expected) => *expected == importance,
            condition => condition.matches_as(m, addresses_only),
        };
        self.enabled
            && self.known()
            && self.validation().is_ok()
            && self.conditions.iter().all(|condition| match condition {
                Condition::Unless(exceptions) => !exceptions.iter().any(ordinary),
                condition => ordinary(condition),
            })
    }

    /// `When from contains "x" and has an attachment: flag it, mark it as read`.
    pub fn describe(&self) -> String {
        self.describe_as(false)
    }

    /// [`Rule::describe`] for a rule that reads addresses alone.
    pub fn describe_as(&self, addresses_only: bool) -> String {
        let when = self.conditions.iter().map(|c| c.describe_as(addresses_only)).collect::<Vec<_>>().join(" and ");
        let then = self.actions.iter().map(Action::describe).collect::<Vec<_>>().join(", ");
        format!("When {when}: {then}")
    }

    pub fn validation(&self) -> Result<(), String> {
        let ordinary = self.conditions.iter().filter(|c| !matches!(c, Condition::Unless(_))).count();
        if ordinary == 0 {
            return Err("a rule needs a positive condition".into());
        }
        if ordinary > 20 || self.conditions.len().saturating_sub(ordinary) > 1 || self.actions.len() > 20 {
            return Err("a rule supports at most 20 conditions, exceptions and actions each".into());
        }
        for condition in &self.conditions {
            condition.validate()?;
        }
        Ok(())
    }

    pub fn reads_body(&self) -> bool {
        self.conditions.iter().any(Condition::reads_body)
    }

    /// A body exception stays unknown until fetched; a matching header exception vetoes it.
    pub fn may_match_headers(&self, message: &Message, addresses_only: bool, now: bool, importance: Importance) -> bool {
        let enabled = self.server.as_ref().filter(|server| !now && !server.id.is_empty()).map_or(self.enabled, |server| server.synced_enabled);
        let header_matches = |condition: &Condition| match condition {
            Condition::Importance(expected) => *expected == importance,
            condition => condition.matches_as(message, addresses_only),
        };
        enabled
            && self.known()
            && self.validation().is_ok()
            && self.conditions.iter().all(|condition| match condition {
                Condition::Unless(exceptions) => !exceptions.iter().any(|exception| !exception.reads_body() && header_matches(exception)),
                condition if condition.reads_body() => true,
                condition => header_matches(condition),
            })
    }

    pub fn targets(&self, folder: Id) -> bool {
        self.actions.iter().any(|a| matches!(a, Action::MoveTo(id) | Action::CopyTo(id) if *id == folder))
    }

    /// Whether it is for the arriving mail of `account`.
    pub fn is_for(&self, account: Id) -> bool {
        self.account == 0 || self.account == account
    }

    /// Whether the server has it.
    pub fn on_server(&self) -> bool {
        self.server.as_ref().is_some_and(|s| !s.id.is_empty())
    }

    /// Whether it is to go to the server and hasn't yet.
    pub fn waiting(&self) -> bool {
        self.keep == Keep::Server && !self.on_server()
    }

    /// Whether every part of it is known here: it can be shown whole and run here.
    pub fn known(&self) -> bool {
        self.server.as_ref().is_none_or(|s| s.unknown.is_empty())
    }

    /// Whether the server doesn't let it be changed or deleted.
    pub fn read_only(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.read_only)
    }

    /// Whether it was switched on or off, or moved, here since the server last had it.
    pub fn changed_here(&self) -> bool {
        self.server.as_ref().is_some_and(|s| !s.id.is_empty() && (self.enabled != s.synced_enabled || s.sequence != s.synced_sequence))
    }
}
