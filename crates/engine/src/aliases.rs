// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Account-scoped virtual alias folders, counted from retained metadata once per revision.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use mailcraft_model::{FolderKind, Id, Mailbox};
use serde::Serialize;

use crate::Session;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alias {
    pub address: String,
    pub total: usize,
    pub unread: usize,
    pub missing_original: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountAliases {
    pub aliases: Vec<Alias>,
    pub messages_with_metadata: usize,
    pub unresolved_alias: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub accounts: BTreeMap<Id, AccountAliases>,
}

/// What one message's SimpleLogin headers say, as far as the inventory needs it: `None` for a
/// message without SimpleLogin metadata; else its receiving alias (if resolved) and whether the
/// original sender is missing.
type Facts = Option<(Option<String>, bool)>;

/// Parsed header facts per message, kept across revisions so that a change to the mailbox
/// (a flag, a move, one new message) does not parse every message's headers again. Keyed by
/// message id and checked against the header count, which changes when headers are fetched.
#[derive(Debug, Default)]
pub(crate) struct FactsMemo {
    by_message: std::collections::HashMap<Id, (usize, Facts)>,
}

fn facts_of(message: &mailcraft_model::Message) -> Facts {
    message.simple_login().map(|identity| (identity.receiving_alias, identity.original_from.is_none()))
}

impl Inventory {
    #[cfg(test)]
    fn of(mailbox: &Mailbox) -> Self {
        Self::of_memo(mailbox, &mut FactsMemo::default())
    }

    fn of_memo(mailbox: &Mailbox, memo: &mut FactsMemo) -> Self {
        let hidden: HashSet<Id> = mailbox
            .folders
            .iter()
            .filter(|folder| matches!(folder.kind, FolderKind::Deleted | FolderKind::Junk))
            .flat_map(|folder| mailbox.subtree(folder.id))
            .collect();
        // Drop what is remembered of messages that are gone, so the memo cannot outgrow the mailbox.
        if memo.by_message.len() > mailbox.messages.len() {
            memo.by_message.retain(|id, _| mailbox.messages.get(id).is_some());
        }
        let mut aliases: BTreeMap<(Id, String), Alias> = BTreeMap::new();
        let mut accounts: BTreeMap<Id, AccountAliases> = BTreeMap::new();
        for message in mailbox.messages.values().filter(|message| !hidden.contains(&message.folder)) {
            let headers = message.headers.len();
            let facts = match memo.by_message.get(&message.id) {
                Some((at, facts)) if *at == headers => facts.clone(),
                _ => {
                    let facts = facts_of(message);
                    memo.by_message.insert(message.id, (headers, facts.clone()));
                    facts
                }
            };
            let Some((receiving_alias, missing_original)) = facts else { continue };
            let account = accounts.entry(message.account).or_default();
            account.messages_with_metadata = account.messages_with_metadata.saturating_add(1);
            let Some(address) = receiving_alias else {
                account.unresolved_alias = account.unresolved_alias.saturating_add(1);
                continue;
            };
            let alias = aliases.entry((message.account, address.to_ascii_lowercase())).or_insert_with(|| Alias { address, ..Default::default() });
            alias.total = alias.total.saturating_add(1);
            alias.unread = alias.unread.saturating_add(usize::from(!message.read));
            alias.missing_original = alias.missing_original.saturating_add(usize::from(missing_original));
        }
        for ((account, _), alias) in aliases {
            accounts.entry(account).or_default().aliases.push(alias);
        }
        Inventory { accounts }
    }
}

impl Session {
    /// Derived metadata only: no message body/attachment reads and no network or provider calls.
    /// Recounted once per revision; each message's headers are parsed only when first seen or
    /// when its headers change ([`FactsMemo`]).
    pub fn aliases(&mut self) -> Arc<Inventory> {
        let key = (self.rev(), self.mailbox.messages.len());
        if let Some((at, inventory)) = &self.aliases_cache
            && *at == key
        {
            return inventory.clone();
        }
        let inventory = Arc::new(Inventory::of_memo(&self.mailbox, &mut self.aliases_memo));
        self.aliases_cache = Some((key, inventory.clone()));
        inventory
    }
}

#[cfg(test)]
#[path = "aliases_tests.rs"]
mod memo_tests;
