# Local mail rules

> **Last reviewed:** 2026-11-07 · **Last updated:** 2026-11-08 · **Change:** minor

Open a destination folder's context menu and choose **Rules for This Folder**.
A settings icon also appears beside a folder with assigned rules, including
disabled rules. Both open the same editor. It lists only rules that move or copy
mail to that exact folder; identical names in another account remain separate.
The heading identifies the destination account and path. New rules start with
that destination. An existing rule may be redirected elsewhere; after saving,
it leaves this destination's list. A rule with several destinations retains one
definition and shows its full action list.

Choose the incoming account, name the rule, add conditions and actions, and use
**Preview** before saving. All ordinary conditions must hold. Any condition marked
**Exception** prevents the whole rule. Rules run in their account's full order,
including rules for other destinations. **Stop processing more rules** stops
after a match, including a match whose action fails.
The folder list shows each rule's position in its incoming account's complete
order. An ordering arrow names the neighboring rule it crosses, including a
neighbor assigned to another destination; filtering does not change execution order.

**Check conflicts** compares the draft with enabled rules for its incoming accounts,
including other destination folders and a server rule whose disable has not reached
the server yet. The report names exact destination paths and accounts. Proven
overlaps claiming different Move destinations prevent creation, editing or enabling;
disabled originals can still be retained for repair. Same destinations and explicit
copies are allowed. Priority and Stop do not hide competing destinations. Narrow
the conditions, add an exception, or disable the conflicting definition.

Complex and regex overlaps that the bounded constructive check cannot prove are
shown as **potential**. This is not a guarantee that all future messages are
conflict-free. Every execution also checks the actual message before any action,
including competing moves made possible by earlier importance changes. A conflict
leaves the message unmoved and unmodified except for its durable diagnostic status.
One diagnostic represents the blocked message's conflict group; matching-rule and
conflicting-message counts remain separate. Ordinary sequential non-conflicting
rules keep their action order and Stop behavior.

Local classification runs when sync first detects a received message, including
initial downloads, older mail and custom or archive receiving folders, across
IMAP, Microsoft Graph, Exchange and JMAP. Sync may poll for mail; classification
does not need its own periodic sweep. Existing provider identity and placement
deduplication prevent a repeated page or fetch from executing filing again.
Sent mail, drafts, trash, junk, their children and shared mailboxes keep their
existing semantics; sender blocking still takes precedence. Classifying mail
already stored before these rules requires an explicit existing-mail pass.

If a potentially applicable rule needs a body or complete cross-account copy,
classification waits for full content instead of matching a short server preview.
Its state survives save/reopen. Required Microsoft/Exchange content can be fetched
outside the ordinary offline age window; JMAP can fetch pending content outside its
newest Inbox selection. Download size limits and provider refusals remain enforced.
Unavailable content remains explicitly pending rather than reported as classified.
The full-content result resumes only that pending message; ordinary reads do not
rerun rules. An explicit relocation cancels pending automatic classification.
`rules.pending` reports complete pending/error totals with bounded, marked detail
rows and content-availability flags. `rules.conflicts` provides the same read-only
cross-destination analysis as the editor, through authenticated MCP.

Simple sender-to-folder rules consolidate automatically when they have the same
incoming account, exact destination, enabled state and Stop
setting. The resulting local rule has **From contains any of**, with one sender
text per line. These are OR alternatives using the same Unicode lowercase
substring matching as **From contains**; they are not regexes. Edit them in the
destination folder's existing editor. Originals remain in the rule's
`mergedFrom` provenance, and command results identify retired ids in `mergedIds`.
Server-owned definitions, exceptions, extra actions, deleting rules, different
accounts/destinations and unproven intervening effects stay separate. Up to 200 sender
alternatives/original definitions fit one group; beyond that the exact separate
rules remain. No source is truncated. Adjacent groups can combine immediately.
Nonadjacent groups can combine across rules for independent accounts. Exact-address
alternatives can also combine when every intervening rule in their scope has a
known positive exact-sender condition disjoint from the later alternatives.
Other rules keep their relative positions and actions. Unknown, overlapping or
order-sensitive effects prevent that merge, including unproven substring or regex
conditions in an intervening rule.

`rules.add` accepts `runIn` for the optional run over existing mail. It runs only
the new rule's predicate before consolidation, returning its `execution` result.
The editor and `rules.fromMessage` use this behavior; adding a sender never
incidentally runs the group's older sender alternatives.

Text fields offer literal matching, **Regex** and **Case sensitive**. Legacy
literal matching uses Unicode lowercase comparison. Case-sensitive literals use
exact substring matching. Regex uses the Rust [regex syntax](https://docs.rs/regex/latest/regex/#syntax),
including Unicode character classes and simple case folding. The case toggle sets
the default; inline regex flags can override it within the expression. Unsupported
lookarounds, backreferences, malformed expressions and compiler resource overflow
produce an error. They are never interpreted as literals. Legacy text supports
200 characters; advanced patterns support 4096 UTF-8 bytes. Limits reject an
oversized definition rather than changing it. There may be up to 20 conditions,
20 exceptions and 20 actions. Body matching includes retained hidden message text.

**Sender address is one of** (`fromAddresses`) and **To/Cc address is one of**
(`toAddresses`) preserve exact transport-address predicates during Outlook
migration. Supply one plain email address per line, up to 200 alternatives and
254 UTF-8 bytes per address. Matching ignores ASCII case and never reads display
names, substrings, domains of longer addresses or a guessed original sender.
New definitions with these predicates stay local. Existing server-owned exact
sender/recipient conditions are inspected without rewriting their provider
definitions. Compatible exact-sender filing rules merge
automatically with exact matching recorded in their original-definition
provenance; they never merge with substring definitions. SimpleLogin original
sender/alias normalization is a separate feature and must not silently change
these migrated transport identities.

Preview scans the selected folder without changing mail or rules. It uses the same
matching, order and action implementation as execution. Its total covers the whole
folder; detailed rows are bounded and explicitly marked when truncated. Previewed
cross-account placement is a plan, not proof that a remote copy exists.
An unavailable or partial body is never treated as complete text, including for
exceptions. Manual execution and preview report the affected message without
performing any of its actions. Fetch its complete content and explicitly rerun;
a manual request is not silently scheduled as a different automatic operation.

Execution reports matches separately from completed rules and failed actions.
Failure details identify the rule/message and how many earlier actions completed.
A missing or unsafe destination pauses a local rule before any action, including
a later Delete. Invalid saved local definitions, including missing literal values
or malformed expressions, also pause with their original conditions intact.
Repairing preserves its id, order and disabled state; enable it
explicitly after repair. Remote placement failures continue through the existing
Send/Receive problems report. Cross-account moves retain their original until
destination placement succeeds. Do not run migrated rules alongside another
client's equivalent filing rules without a deliberate migration decision.

Deleting a destination folder removes the complete rules that move or copy mail
to it or its deleted descendants, including rules whose source is another
account. Moving a custom folder under Deleted Items has the same effect. This
does not run those rules or delete any additional messages. Undo restores the
folder and its definitions together while the change remains reversible.
Confirmed provider folder changes use the same cleanup; an incomplete or failed
listing does not establish deletion. A merely unknown destination still pauses
the definition instead of removing it. Intentional Delete actions and rules
targeting the Deleted Items root remain separate email-deleting operations.

A provider-owned definition stays in the durable pending-deletion list until the
provider acknowledges removal or a complete listing confirms absence. An old
download cannot resurrect it. Definitive refusals remain visible through
`rules.list` in `pendingDeletions`, with `blocked` describing the unmet provider
condition; they are not silently reported as successful or automatically replayed.
`pendingDeletionCount` covers every matching record; detailed entries are bounded
and `pendingDeletionsTruncated` reports omitted detail. Folder discovery also
exposes provider identities separately from display names. `imapPath` preserves
such differences as a trailing space. `name` retains the exact provider label when
known; null means it was not retained, including a label over the reader's bound.
`recordedName` is the synchronization comparison name and may already be
normalized. Exact names do not change the display or queue a rename. Provider
labels are untrusted data; none of these fields exposes private sync state.

Cross-account filing and copies retain `filingSourceAccount` in local mailbox
metadata, separately from the current `account`. It records the account observed
before the first filing by this mailbox and survives persistence and provider
source removal. It is not an outbound header or proof of an earlier delivery
history. Historical mail with no such observation stays unknown; no source is
guessed from display names or recipient text. Later filing preserves the recorded
source, and native mail inspection exposes both identities.

`mail.filingStatus {"id": number}` inspects one message's recorded provider
identity, pending destination placement and pending source removal without loading
its body, fetching mail, running synchronization or changing the selection. A
pending placement does not confirm a remote copy. `recordedServer` is the last
native provider observation, not a new server lookup; `serverLookupPerformed`
is false. The source account remains separate from the current storage account.
The response contains no credentials or placement replay token. Actual save
completion and provider acknowledgements remain separate migration criteria.
Content availability is separate too: an old message can retain its confirmed
destination while the account's offline policy keeps only its headers and preview.
Inspect `content` explicitly and request the full message through the connected
app before claiming a full read. Startup retention waits for opening content
checks and body handoff, so it cannot remove files those checks are still using.

Regex, explicit case matching, exceptions and copy actions run locally while
MailCraft is open, including its headless mail runtime. Server-owned originals
remain intact and are not rewritten as simpler local definitions. Unknown server
operations remain visible as unsupported. Exceptions are stored as an explicit
condition type, so older readers cannot silently ignore a new optional field and
broaden a rule. Use a compatible version when opening mailboxes with new rule types.

Native `rules.add`, `rules.edit`, `rules.list`, `rules.preview` and `rules.run`
describe their parameters through command discovery. Listing and non-mutating
previews are read-only MCP operations and work headlessly. Ordinary filing rules
can be created, edited, enabled and run through MCP without owner prompts, including
moves between the configured accounts. Creating, editing, enabling, reordering or
running email-deleting rules requires owner consent, locally and on the server.
Moving mail into Deleted Items or a folder beneath it counts as deletion. Disabling
or removing a rule definition does not delete mail and needs no prompt. Unsupported
server effects remain protected until their behavior is known. Provider OAuth and
unrelated account, sending and destructive-action approvals remain in place.
Use the existing authenticated bridge and installed workflow gate. Preview is not
permission to execute a rule, change settings or bypass an owner question.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-08 | patch | Explain durable filing identity, offline retention and startup content-check ordering. |
| 2026-11-08 | minor | Document per-message filing-state inspection and the distinction between recorded provider state and a fresh server lookup. |
| 2026-11-07 | minor | Document local matching, destination-scoped management, explicit action failures, safe exception storage, automatic sender consolidation and action-based MCP consent. |
