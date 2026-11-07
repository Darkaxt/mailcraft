// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rule-definition removal, including the durable work still owed to a provider.

use std::collections::{HashMap, HashSet};

use crate::{Action, FolderKind, Id, Mailbox, Rule, RuleGone};

impl Mailbox {
    /// Remove a definition, never execute it or remove mail. Keep a provider deletion until
    /// acknowledged: dropping intent because an unrelated queue reached a fixed length can
    /// resurrect a deleted rule on the next download.
    pub fn remove_rule_definition(&mut self, index: usize) -> Option<Rule> {
        let rule = self.rules.get(index)?.clone();
        let remote = rule.server.as_ref().filter(|server| !server.id.is_empty());
        let id = remote.map_or_else(String::new, |server| server.id.clone());
        if remote.is_some() || rule.unanswered {
            let already = self.rule_gone.iter().any(|gone| {
                gone.account == rule.account && if id.is_empty() { gone.made.as_ref().is_some_and(|made| made.id == rule.id) } else { gone.id == id }
            });
            if !already {
                self.rule_gone.push(RuleGone {
                    account: rule.account,
                    id,
                    made: remote.is_none().then(|| Box::new(rule.clone())),
                    blocked: if rule.read_only() { "the server marks this rule read-only".into() } else { String::new() },
                });
            }
        }
        Some(self.rules.remove(index))
    }

    /// Only call with identities whose deletion is known, or whose move into Deleted Items
    /// was observed. A missing/unreadable destination alone must never infer deletion.
    /// Remove the whole definition: retaining its other actions could change its meaning.
    pub fn remove_rules_for_deleted_folders(&mut self, folders: &[Id]) -> usize {
        let folders: HashSet<Id> = folders.iter().copied().collect();
        let mut removed = 0;
        for index in (0..self.rules.len()).rev() {
            if self.rules.get(index).is_some_and(|rule| {
                rule.actions.iter().any(|action| matches!(action, Action::MoveTo(id) | Action::CopyTo(id) if folders.contains(id)))
            }) {
                let _ = self.remove_rule_definition(index);
                removed += 1;
            }
        }
        removed
    }

    /// Custom folders below Deleted Items, excluding the intentional deletion root itself.
    /// Parent identities, not display names, determine the relationship.
    pub fn deleted_destination_folders(&self) -> HashSet<Id> {
        let mut children: HashMap<Id, Vec<&crate::Folder>> = HashMap::new();
        for folder in &self.folders {
            if let Some(parent) = folder.parent {
                children.entry(parent).or_default().push(folder);
            }
        }
        let mut stack: Vec<_> = self.folders.iter().filter(|folder| folder.kind == FolderKind::Deleted).collect();
        let mut seen = HashSet::new();
        let mut deleted = HashSet::new();
        while let Some(folder) = stack.pop() {
            if !seen.insert(folder.id) {
                continue;
            }
            if folder.kind == FolderKind::Custom {
                deleted.insert(folder.id);
            }
            stack.extend(children.get(&folder.id).into_iter().flatten().copied().filter(|child| child.account == folder.account));
        }
        deleted
    }
}
