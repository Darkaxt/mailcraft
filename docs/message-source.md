# Message source and .eml export

The native reading pane has a **Source** button above the message header.
It replaces the rendered body with selectable, fixed-width HTML source.
HTML is indented for reading; plain-text messages are labeled **Plain-text source**.
Use **Message** to return to the normal reading view.

Source is text: scripts, links, image addresses and style declarations do not
run or download resources. Control and invisible formatting characters are
shown visibly. The stored body is unchanged. Long source is paged, with explicit
byte ranges and Previous/Next controls rather than an undisclosed cutoff.

**Export .eml** prepares the complete message before asking where to save it.
The native exporter streams retained bodies and attachments, verifies stored
parts against their SHA-256 and length, and atomically replaces the chosen file
only after the complete output is available. Canceling the picker releases its
private preparation. Missing, damaged or incomplete content produces an error
instead of a file presented as complete.

MailCraft currently retains decoded bodies, modeled/extra headers and attachment
content, not the original MIME byte stream. The exported RFC 5322/MIME file is
therefore **reconstructed**, including known recipient and reply headers,
plain/HTML alternatives, inline content IDs and attachments. It is not a
byte-identical original and cannot recreate original MIME nesting, transfer
encoding or a cryptographic signature over those original bytes.
Saved drafts retain known Bcc recipients; sent and received message exports keep
MailCraft's existing Bcc omission. Delivery-status attachments use their required
7-bit MIME encoding and preserve retained bytes; invalid 7-bit content fails the
export rather than producing an invalid complete file.

## MCP and headless access

Use the existing `execute` tool with `mail.sourcePage` or `mail.exportEml`:

1. Open with `{"id": 123}`. Keep the returned `token` and `state: "pending"`.
2. Read that same token with `{"id": 123, "token": 1, "offset": 0, "limit": 32768}`.
   Pending means preparation is still running; observe the same token.
3. A ready response includes total `bytes`, whole-snapshot `sha256`, `eof` and
   `nextOffset`. Continue from that returned offset until `eof` is true.
4. Close with `{"id": 123, "token": 1, "close": true}`. If the response is
   `closing`, observe that same close through `closed`; no timeout kills its worker.

Limits are 4–65,536 input bytes per page. Source pages are UTF-8 aligned visible
text, framed as untrusted mail content by MCP. `.eml` pages contain Base64 `data`;
decode and append each chunk before comparing the complete byte length and hash.
The hash/offsets for formatted source describe the snapshot before invisible
characters are made visible, not concatenated MCP framing or display text.
`.eml` reports `provenance: "reconstructed"` and `originalMimeAvailable: false`.

Snapshots are local to one message and mailbox opening. There is one preparation
worker and at most four owned snapshots; close readers when finished. The UI
releases its readers when leaving Source or changing selection. Native temporary
files close with their owners; no credentials or native snapshot paths are returned.

`mail.viewSource` remains compatible with its existing header/body response and
explicit one-megabyte truncation flag. Its native engine response remains plain
text; MCP frames the `source` string as untrusted mail text, like other message
fields. `file.exportMessage` still supports normal
native path export and small whole Base64 responses; whole native responses are
limited to 4 MiB so they fit the control transport. For larger programmatic
exports use `mail.exportEml` chunks. A ready EML token can also be passed to
`file.exportMessage` for a native owner-selected path. Existing MCP filesystem
restrictions still apply. Browser View Source and Save As remain available;
the new native snapshot/paging interface requires the native app.
