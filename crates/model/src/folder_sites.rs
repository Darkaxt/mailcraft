// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder's site (0.12.34): the one web site nearly all of a folder's mail comes from, whose
//! icon the folder pane shows before the folder's name when the owner gave it no emoji of its own.
//!
//! - A sender's site is its address's registrable domain ([`crate::provider::site`]:
//!   `coinbase.com` for `no-reply@info.coinbase.com`); mail forwarded by SimpleLogin counts as
//!   from the original sender it reports, not from the alias service.
//! - The owner's own addresses don't count: their replies are in many folders.
//! - One brand's sites in several countries count as one (`amazon.es`, `amazon.de`, `amazon.com`:
//!   the label before the suffix); the folder wears the icon of the one most of its mail is from.
//! - At least [`MIN_MESSAGES`] counted, and at least nine in ten of them from the brand
//!   ([`SHARE`]); a folder with fewer wears its parent's (`Amazon/Returns`), one with more from
//!   several, none.
//! - Never a mail provider's (`gmail.com`: people, not a brand), nor a name on a hosting platform.

use std::collections::{HashMap, HashSet};

use crate::{Message, provider};

/// What the window hands the host for a site's icon, before the site: `site:coinbase.com` (an
/// address has no `site:` in it).
pub const ICON_KEY: &str = "site:";

/// Fewest messages a folder has its own site for.
pub const MIN_MESSAGES: usize = 3;
/// The share of a folder's mail that must be from one brand: nine in ten.
pub const SHARE: (usize, usize) = (9, 10);

/// What a folder's mail says of its site.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nearly all of it is from this site.
    Site(String),
    /// From several: no icon.
    Mixed,
    /// Too little to tell: its parent's.
    TooFew,
}

/// The site `m` is from: its SimpleLogin original sender's, else its sender's. `None` for one of
/// `own` (addresses in lower case) or no address.
pub fn sender_site(m: &Message, own: &HashSet<String>) -> Option<String> {
    let original = m.simple_login().and_then(|identity| identity.original_from).map(|a| a.email);
    let address = original.unwrap_or_else(|| m.from.email.clone()).trim().to_lowercase();
    if own.contains(&address) {
        return None;
    }
    let (_, domain) = address.rsplit_once('@')?;
    let domain = domain.trim_end_matches('.');
    let site = provider::site(domain);
    // A forward whose original sender SimpleLogin didn't say is from nobody known.
    (provider::is_domain(domain) && site != "simplelogin.co").then(|| site.to_string())
}

/// The brand a site is of: its label before the suffix (`amazon` for `amazon.co.uk`).
pub fn brand(site: &str) -> &str {
    site.split('.').next().unwrap_or(site)
}

/// What `messages` say of their folder's site.
pub fn verdict<'a>(messages: impl IntoIterator<Item = &'a Message>, own: &HashSet<String>) -> Verdict {
    let mut sites: HashMap<String, usize> = HashMap::new();
    let mut counted = 0usize;
    for m in messages {
        if let Some(site) = sender_site(m, own) {
            *sites.entry(site).or_default() += 1;
            counted += 1;
        }
    }
    if counted < MIN_MESSAGES {
        return Verdict::TooFew;
    }
    let mut brands: HashMap<&str, usize> = HashMap::new();
    for (site, n) in &sites {
        *brands.entry(brand(site)).or_default() += n;
    }
    let Some((top, n)) = brands.into_iter().max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0))) else { return Verdict::TooFew };
    if n * SHARE.1 < counted * SHARE.0 {
        return Verdict::Mixed;
    }
    // The brand's site most of it is from (the first by name between equals).
    let site = sites.iter().filter(|(s, _)| brand(s) == top).max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))).map(|(s, _)| s.clone());
    match site {
        Some(site) if wears_an_icon(&site) => Verdict::Site(site),
        _ => Verdict::Mixed,
    }
}

/// Whether a site's icon says something of a sender: not a mail provider's, a public suffix, or a
/// name on a hosting platform.
pub fn wears_an_icon(site: &str) -> bool {
    provider::is_domain(site) && !provider::is_mail_provider(site) && !provider::is_public_suffix(site) && !provider::on_hosting_platform(site)
}
