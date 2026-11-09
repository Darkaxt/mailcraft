// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mailbox-owned saved queries. These are never provider folders or filing destinations.

use crate::Id;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchScope {
    /// The exact real folder in view.
    #[default]
    Folder,
    /// That folder and its actual descendants.
    Subfolders,
    /// The anchor folder's account, excluding Junk/Deleted descendants.
    Mailbox,
    /// All accounts except Junk/Deleted descendants.
    Everywhere,
    /// The virtual union of every account's real Inbox, including without a query.
    Inboxes,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchFolder {
    pub id: Id,
    pub name: String,
    pub query: String,
    pub scope: SearchScope,
    /// Exact real anchor for folder/subfolder/mailbox scope, never a basename guess.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<Id>,
    /// Explicit canonical Outlook tree when saved from a unified filing view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filing: Option<Id>,
}

pub const MAX_SEARCH_FOLDERS: usize = 200;
