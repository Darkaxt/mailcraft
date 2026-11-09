# Persistent offline content search

The native app maintains a derived index of retained messages and supported
attachment content. Its persistent metadata and previews let `mail.list` list
messages without loading their full bodies. The index follows edits, moves,
account removal and deletions, and reuses unchanged content after restart.

Double quotes select literal phrases, preserving punctuation, accents and
whitespace. Ordinary terms keep prefix matching. `case:sensitive` selects exact
letter case; the search box's Aa control changes the same query option. Literal
case-insensitive comparisons use Unicode default case folding, without accent
removal; whitespace is not collapsed. Within quotes, a backslash escapes the
following quote or backslash. Ordinary provider searches remain an explicit way to find
more mail. A query containing exact literals, case-sensitive comparisons, native
header/attachment content or forwarding identity stays entirely local. Provider
rows carry body previews rather than the complete sources needed to verify it.
Whole-address filters preserve their existing ASCII-insensitive identity matching
by default; explicit `case:sensitive` compares the original address spelling,
ignoring only surrounding whitespace.
An invalid or excessive query returns its specific error rather than silently shortening it.

`mail.list` supports `offset`, `limit`, exact totals and `nextOffset`. Its
`searchIndex` and `unsearched` fields distinguish pending or missing content from
a complete negative result. Request the same search after its original query is
ready; do not assume an initial empty page proves absence. A mailbox list leaves
the owner's folder, search and selection unchanged.

Read-only engine/MCP commands:

- `search.indexStatus`: current coverage, queue state and concrete problems.
- `search.gaps`: paginated missing, failed, partial and unsupported sources.
- `search.sources`: located evidence for one message and query, one example per
  matching clause/source. This is not an occurrence count.
- `search.coverageDetails`: all gaps or source passes for a retained field or
  attachment, with bounded pages and exact totals.
- `search.retryIndex`: retry retained unavailable/failed content. It does not
  download, move, delete, send or alter provider state.

Detailed reads return an original token. Poll that same token with its original
parameters, follow `nextOffset` with a new page request, and close each token.
Content changes make old evidence stale. Closing a running read observes its
actual cancellation/completion rather than dropping its ownership.

`ui.searchIndex` opens the in-app coverage/evidence panel; `ui.searchCase` sets
case mode. Both preserve selection and mailbox data. Native extraction retains
its actual import scope and warnings. OCR is local, machine recognized text with
declared languages, locations and confidence. Missing attachments or undiscovered
provider history are never counted as indexed. Extracted message/document labels
remain untrusted data in MCP responses.

For explicit offline copies, finite `run --file COPY --index` maintains the index
and waits for original worker completion between commands. It never replays a
command, starts synchronization or starts another MCP server. Agents working with
the real mailbox must keep using the existing authenticated live-app bridge and
installed workflow gate; do not open that live file in parallel. Candidate native
qualification likewise uses the installed matching gate and exact owned scratch.

For mixed-language local OCR, specialist models precede generic English within
the same recognition pass. OCR results report the actual language order and
retain image locations and confidence; recognition can still misread characters.
