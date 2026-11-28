// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The site a folder's icon is of (0.12.34, [`mailcraft_model::folder_sites`]), for the folder
//! pane: worked out from a folder's mail when the pane first shows it, and again only once the
//! number of messages in it changed.

use std::collections::HashSet;

use mailcraft_model::folder_sites::{Verdict, verdict};
use mailcraft_model::{FolderKind, Id};

use crate::Session;

/// Most folders up a folder too small to tell borrows its parent's site from.
const MAX_UP: usize = 8;

impl Session {
    /// The site whose icon folder `id` wears: its own mail's, else (too little of it to tell) its
    /// parent's; a twin in the Local archive wears the icon of the folder it is the twin of. Only
    /// folders the owner made (not an Inbox, Sent Items, Junk E-mail...).
    pub fn folder_site(&mut self, id: Id) -> Option<String> {
        let mut cur = id;
        for _ in 0..=MAX_UP {
            let f = self.mailbox.folder(cur)?;
            if f.kind != FolderKind::Custom {
                return None;
            }
            let (parent, twin_of) = (f.parent, f.twin_of);
            match self.folder_verdict(cur) {
                Verdict::Site(site) => return Some(site),
                Verdict::Mixed => return None,
                Verdict::TooFew => cur = twin_of.or(parent)?,
            }
        }
        None
    }

    /// The sites the owner's folders wear, by brand (`amazon` → `amazon.es`): what a sender of
    /// one of them shows on a row of the Inbox (v0.12.47), with no site looked up that the folder
    /// pane doesn't already show.
    pub fn folder_site_brands(&mut self) -> std::collections::HashMap<String, String> {
        let ids: Vec<Id> = self.mailbox.folders.iter().filter(|f| f.kind == FolderKind::Custom).map(|f| f.id).collect();
        let mut brands = std::collections::HashMap::new();
        for id in ids {
            if let Verdict::Site(site) = self.folder_verdict(id) {
                brands.entry(mailcraft_model::folder_sites::brand(&site).to_string()).or_insert(site);
            }
        }
        brands
    }

    /// What folder `id`'s own mail says, kept while the number of messages in it stays the same.
    fn folder_verdict(&mut self, id: Id) -> Verdict {
        let counts = self.folder_counts();
        let ids = counts.ids(id);
        if let Some((n, said)) = self.site_cache.get(&id)
            && *n == ids.len()
        {
            return said.clone();
        }
        let own: HashSet<String> = self.mailbox.accounts.iter().map(|a| a.email.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
        let said = verdict(ids.iter().filter_map(|m| self.mailbox.message(*m)), &own);
        self.site_cache.insert(id, (ids.len(), said.clone()));
        said
    }
}
