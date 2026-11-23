# Fork plan: our own Android mail client

> **Status:** proposed, not started (2026-11-24) · owner's decision pending on the base

The fork has so far shipped Windows x64 only. This records the plan for a phone client of our own,
so the decision and its reasons are not lost before work starts.

## Decision to make

Which project the Android client is built from:

| | MailCraft's own Android app (`apps/mailcraft-android`) | FairEmail ([m66b/fairemail](https://github.com/m66b/fairemail)) |
|---|---|---|
| Engine | The same Rust core as the Windows app (`crates/mobile`, UniFFI) | Its own, in Java |
| Microsoft accounts | Microsoft Graph, as on Windows | IMAP/POP3/SMTP only; no EWS, ActiveSync or Graph |
| Our customizations | Mostly come for free: they live in the engine | Every one written again, in a second codebase |
| Maturity | Young: builds, unit and screenshot tests, weekly emulator end-to-end test; not used daily yet | Mature (since 2018, ~32k commits), but large and option-heavy |
| Licence | Same as this repository | GPL-3.0: code copied from it makes the result GPL |
| Interface | Its own Jetpack Compose screens (not the Windows egui UI) | Its own |

**Recommendation:** MailCraft as the base, FairEmail as a reference for what it has proven on
Android: push without draining the battery (IMAP IDLE, Doze, background limits), notification
grouping, privacy defaults (remote images, link checks, tracker blocking).

## Customizations the phone must have

What the Windows fork already does that the phone app must honour (most of it is in the engine):

- The unified mailbox and its tree; folder tags; folder site icons (`folder_sites`).
- Rules: moving mail needs no question; deleting mail needs the owner's yes.
- The Local archive behaving as an archive (packed while unused, counts kept).
- Test mail only among the owner's own accounts.

## Target device: Galaxy Z Fold7

The owner's phone. Design for the open inner screen first, the cover screen second.

- **Inner screen open:** 8.0", 2184 × 1968, almost square (about 10:9; wider than tall when
  held sideways). Android sizes this as an *expanded* window. Use it like the Windows window:
  the folder tree, the message list and the reading pane side by side, not a stretched phone
  list. Held upright, two panes (the list and the message), with the folder tree as a drawer.
- **Cover screen:** 6.5", 2520 × 1080, 21:9, tall and narrow. One pane at a time (list, then
  message), for quick triage.
- **Folding and unfolding:** the app keeps the open message, selection and scroll position across
  the change and does not reload.
- **Half folded (Flex mode):** the message on the top half and the list or the reply box on the
  bottom half.
- Use Android's adaptive layouts (window size classes, Jetpack WindowManager for the fold), not
  a check on the model name. The emulator has a matching foldable profile for screenshot tests.

## First steps (when it starts)

1. Build the upstream Android app unchanged (Gradle gate) and try it with one account on an
   emulator or the owner's phone (the owner says which; device access through the Android gate).
2. List what is missing against the list above, and what FairEmail does better in daily use.
3. Add what is missing to the shared engine first; screens only after.

## Meanwhile

CI keeps its `android` and `android-app` jobs so the phone app keeps compiling; releases stay
Windows x64 only until this starts.
