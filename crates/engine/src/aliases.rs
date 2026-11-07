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

impl Inventory {
    fn of(mailbox: &Mailbox) -> Self {
        let hidden: HashSet<Id> = mailbox
            .folders
            .iter()
            .filter(|folder| matches!(folder.kind, FolderKind::Deleted | FolderKind::Junk))
            .flat_map(|folder| mailbox.subtree(folder.id))
            .collect();
        let mut aliases: BTreeMap<(Id, String), Alias> = BTreeMap::new();
        let mut accounts: BTreeMap<Id, AccountAliases> = BTreeMap::new();
        for message in mailbox.messages.values().filter(|message| !hidden.contains(&message.folder)) {
            let Some(identity) = message.simple_login() else { continue };
            let account = accounts.entry(message.account).or_default();
            account.messages_with_metadata = account.messages_with_metadata.saturating_add(1);
            let Some(address) = identity.receiving_alias else {
                account.unresolved_alias = account.unresolved_alias.saturating_add(1);
                continue;
            };
            let alias = aliases.entry((message.account, address.to_ascii_lowercase())).or_insert_with(|| Alias { address, ..Default::default() });
            alias.total = alias.total.saturating_add(1);
            alias.unread = alias.unread.saturating_add(usize::from(!message.read));
            alias.missing_original = alias.missing_original.saturating_add(usize::from(identity.original_from.is_none()));
        }
        for ((account, _), alias) in aliases {
            accounts.entry(account).or_default().aliases.push(alias);
        }
        Inventory { accounts }
    }
}

impl Session {
    /// Derived metadata only: no message body/attachment reads and no network or provider calls.
    pub fn aliases(&mut self) -> Arc<Inventory> {
        let key = (self.rev(), self.mailbox.messages.len());
        if let Some((at, inventory)) = &self.aliases_cache
            && *at == key
        {
            return inventory.clone();
        }
        let inventory = Arc::new(Inventory::of(&self.mailbox));
        self.aliases_cache = Some((key, inventory.clone()));
        inventory
    }
}
