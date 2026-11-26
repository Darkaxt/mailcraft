# For you: surfacing the mail worth reading (fork design)

Date: 2026-11-26. Fork-only feature (Darkaxt/mailcraft); keep it in separate modules so upstream syncs stay cheap.

## Why

The owner reads mail through the unified views and neither works:

- **Unified Inbox** misses everything his rules file into folders (Travels, Security, Banks...), including
  things that matter now: Iberia tickets, OTP codes.
- **Unified All mail** is buried in automated noise. On 2026-11-26 its top 11 rows were all automated: GitHub
  CI notifications about his own pushes, a Codex bot, and Steam/Booking/Answear/Etsy/XDA/LinkedIn/Simkl mailings.
  It shows 66,156 unread: "unread" means "never cared to open", not "not seen yet".

Goal: put what he should look at first above the date-sorted list, without him curating anything by hand.

## Goals

1. A live **For you** section at the top of the message list in **All mail** and in **Inbox** (unified and per
   account), above the usual date groups.
2. **All mail's For you**: unread mail ranked in tiers (below).
3. **Inbox's For you**: unread mail that sits in folders other than the Inbox (filed by rules), surfaced back.
4. A **settings area** for For you to adjust its rules.
5. **Reading pane auto-hides** when nothing is selected; the list takes the full width.
6. **Folder & subject rows**: a row layout led by the folder (icon and name) with a larger subject, no sender, no preview.

## Non-goals

- Not pins. The existing manual `pinned` flag and the "Pinned" group are untouched; For you never sets it.
- No machine learning, no cloud service, no sending anything anywhere. Everything is computed locally.
- No change to rules, folders or the read state of any message. For you only lists.
- No change to the other folders' lists (only All mail and Inbox get the section).

## Anti-goals (failure even if it works)

- For you filling up with noise: promotions, newsletters, bots, mail triggered by his own actions.
- A long For you section that pushes the normal list off the screen.
- Anything that alters mail state on its own (marking read, moving, pinning).

## For you in All mail: tiers

Only unread messages qualify; reading one removes it on the next list build. Order: tier, then newest first.
A message appears once, in its highest tier.

1. **New sender**: the first message from that sender address, received within the last month (window
   adjustable). (Noise exclusions below apply.)
2. **People you reply to**: the sender qualifies when he answered (the `answered` flag) at least one of the
   last 10 messages received from that sender (count adjustable). It drops out by itself once 10 more messages
   arrive unanswered.
3. **Links you open**: the sender qualifies when he opened a link in more than half of the messages received
   from that sender in the last 2 months (window and share adjustable). Needs the new link-open log (below);
   this tier starts empty and fills over weeks.
4. **Senders you read**: the sender qualifies when more than half of the messages received from that sender in
   the last 2 months are read (window and share adjustable). Reads on other devices count (the synced flag).

**Noise exclusions, all tiers** (owner: "your pick"): messages with `List-Unsubscribe`, bot/automated senders
(`Auto-Submitted`, `noreply`-style addresses, `[bot]` names) and messages carrying sender warnings never qualify
for any tier. Tickets and OTPs still reach the Inbox's For you through the per-folder switch, which ignores this.

Also never qualifies, in any tier: mail triggered by the owner himself, i.e. sent from one of his own addresses, or
notification mail about his own activity (e.g. GitHub notifications whose triggering actor is one of his
accounts; recognise via the sender display name matching his own account names on GitHub-style notifications).

## For you in Inbox: filed mail surfaced back

Candidates: unread messages in any folder except Inbox, Sent, Drafts, Deleted Items, Junk, Archive and Outbox
(they got there by a rule or by hand; no rule provenance is stored, and none is needed).
A candidate shows when either:

- it qualifies under the All mail tiers above (automatic), or
- its folder has the per-folder switch **"Show in the Inbox's For you"** on (guarantees e.g. Travels, Security).

Per-folder **age limit** (optional): a message leaves For you once older than the folder's limit, e.g.
30 minutes for an OTP folder. Default: no limit (it leaves when read).

## How the section behaves (builder-check answers; builder's calls unless marked)

- **Moved, not duplicated**: a message shown in For you is left out of its date group below, so keyboard
  navigation, selection and counts see it once.
- **Conversations**: the unit follows the current view. In conversation view a thread shows once, in the
  highest tier of its unread messages, and counts once toward the cap.
- **Sender identity**: the exact sender address (lower-cased), pooled across all accounts, so a person ranks
  the same in unified and per-account views. No domain grouping.
- **New sender** is checked against everything the local mailbox holds (all folders, all accounts). A sender
  he wrote to first still counts as new (their first reply surfaces). Limitation: senders seen only before
  the oldest synced mail look new.
- **Inbox ordering and cap**: mail from folders with the switch on comes first, newest first, and is never
  capped (a fresh OTP must not hide behind "Show all"); tier-qualified mail follows in tier order, under the cap.

## Settings area

Reached from the For you section header (a gear) and from settings. Contents:

- For you on/off, separately for All mail and Inbox.
- Each tier on/off, with its window/count/share.
- The list of folders with their "Show in the Inbox's For you" switch and age limit.
- A cap on how many messages For you shows (default 15; the rest stays reachable via "Show all").
- Clear the link-open history.

## Link-open log

New local record, per message: whether a link in it was opened from MailCraft (the reading pane's link
handling, `apps/mailcraft/src/browser.rs` `OpenUrl` path, attributed to the message on display). Stored with the
mailbox's local (non-synced) state, never sent anywhere, never in backups' shared parts, clearable from the
settings area. Only the fact "opened a link in message X" and its time are stored, not the URL.

## Reading pane auto-hide

When no message is selected the reading pane is hidden and the list uses the full width; selecting a message
shows it again where it was (right or bottom). Applies only when the pane is on (`Pane::Right/Bottom`); "Off"
stays off. Must not reflow the list under the pointer while the owner is clicking (the pane appears after the
selection, the clicked row stays under the pointer).

## Folder & subject rows

A new View option, **"Folder & subject rows"** (alongside upstream's Compact/Single/Preview layouts), on for the
owner. Each row has two lines and no preview text:

```
Today
> [GitHub icon] Github                                       18  22:46
  [Darkaxt/Navic] Run failed: Build Navic - v1.6.48 (939633a)
```

- Top line: the folder's icon and name, small; conversation count and time on the right.
- Bottom line: the subject, in a larger font, using the room the sender line and preview took.
- The sender is not shown in the row; it appears on hover (tooltip). The mail is already classified, so the
  folder says more than the sender.
- Mail not filed into a folder (sitting in an Inbox): the top line shows the sender instead, so every row says
  where it came from.
- The existing View > Message Preview (0-3 lines) stays as it is for the other layouts.

## Approaches considered

- **A (chosen): compute For you while building the list.** One pass over the candidates' senders' recent
  history per list build (about 25k messages here: milliseconds), with a per-sender stats cache invalidated by
  mailbox changes. Only new stored state: the link-open log and the settings.
- B: a per-sender score table updated on every sync. Faster for huge mailboxes but adds stored state that can
  drift and needs repair; not needed at this size.
- C (rejected): reuse the manual pin flag. Mixes with the owner's own pins and would change mail state.

## Decisions

Owner: For you as a live section (not pins); tiers and order; new sender = last month; reply tier = 1 answered
among the last 10 received; links/read tiers over the last 2 months instead of a count; Inbox variant surfaces
filed mail; option C (tiers plus per-folder switch); a settings area; pane auto-hide; hide the preview line and
give the subject more room.

Mine (proposed, accepted by the owner): exclude bulk/bot/warned senders from "new sender"; exclude mail
triggered by himself; manual pins untouched; "usually" = more than half; per-folder age limit; "Folder & subject rows"
as a View option; noise exclusions on every tier; sender on hover only.

## Left to the builder

Exact UI of the section header and settings, sender-stats cache design, the self-triggered-notification
heuristic details, the default cap, test layout. Consent policy: none of this changes mail, so all For you
commands are Safe for agents; changing For you settings is a settings change (owner's).
