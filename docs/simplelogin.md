# SimpleLogin aliases and sender identities

> **Last reviewed:** 2026-11-07 · **Last updated:** 2026-11-07 · **Change:** minor

Expand **Aliases** beneath an account in the folder pane. Each row filters that
account's downloaded mail by its exact receiving alias, across its real folders.
Deleted Items, Junk Email and their descendants stay out. These are virtual
filters: selecting one creates no folder and moves no messages. Counts and
membership come from retained headers, work offline, and are rebuilt after
reopening the mailbox. Expansion follows the saved layout; searches start fresh
at launch, as other searches do. Large alias lists scroll independently.

The reading header shows the original correspondent reported by SimpleLogin,
the receiving alias and the actual reply route. **Header-reported identity**
explains its provenance and exposes the transport sender on hover. MailCraft
does not authenticate an original-sender claim merely because a message has
SimpleLogin headers. Missing, duplicate, invalid or unsupported direction values
remain explicit gaps. No original address is decoded from a reverse alias or
guessed from a display name.

Replies continue to use the actual Reply-To, or the actual From when Reply-To
is absent. Displaying a reported sender never replaces either transport value.
The integration does not create, disable, delete or reroute SimpleLogin aliases,
change provider options, or request account/API access by itself.

Search fields distinguish these identities:

| Search | Meaning |
|---|---|
| `alias:shop@aliases.example` | Exact header-reported receiving alias. |
| `originalfrom:sender@shop.example` | Exact header-reported original sender. |
| `transportfrom:reverse_random@simplelogin.co` | Actual transport sender; equivalent to existing `from:`. |

Alias and original-sender fields require a complete address, compare addresses
without ASCII case differences, and remain local predicates. They are never
substituted for transport fields in a provider search. Existing `from:`, `to:`
and migrated sender rules retain their original meanings. Local rule conditions
**Reported sender contains** and **Receiving alias contains** provide their own
regex and case controls. These remain header claims even when used by a rule;
they are not an authentication or sender-trust policy.

Through the existing authenticated MCP `execute` tool:

- `aliases.list {account, offset?, limit?}` returns complete alias/coverage totals
  and a page of at most 500 aliases. Continue with `nextOffset` when present.
- `mail.identity {id}` returns transport sender, actual reply recipients and
  separately attributed forwarding metadata, without loading a body or attachment.
- `view.alias {account, alias}` opens the same virtual view as its folder-pane row.
- `ui.toggleAliases {account}` expands/collapses that account's alias section.

These operations require no additional mailbox-action grant. Message-derived
identity text remains untrusted data. Coverage refers to downloaded retained
headers, not all messages or aliases on a remote service. An older message whose
forwarding metadata was never supplied or retained is reported as unavailable;
neither a search nor an alias count claims to reconstruct it.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-07 | minor | Document header-reported forwarding identity, virtual account-scoped aliases, unchanged reply routing, explicit local predicates and metadata coverage. |
