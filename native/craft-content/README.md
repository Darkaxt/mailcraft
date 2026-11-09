# Located Craft content

This directory adapts the safe MIT Rust engines reused by Pathweft. It contains
no viewer ABI, application GUI, MCP server, account credentials or settings.
`sources.json` records exact donor archives, font commits and original adapter
hashes. MailCraft's modified source is separately hashed by the build receipt.
Existing Pathweft sources and its rendering interfaces remain unchanged.

The reusable `crates/craft-content` contract is version 1: native text records
have structured locations, UTF-8 byte offsets, derivation and bounded resumable
pages. Its optional Windows runtime owns each finite process tree, verifies
selected payloads, supports explicit cancellation and observes actual exit.
On Windows, process-wrap supplies suspended creation and resumption. A separate
cosca job is bound before resumption and retained until its live membership and
member-handle waits confirm exit; process-wrap 10.0.1's first-notification wait is
not used as completion evidence. A separately closable cancellation job can end
the exact tree while the observation job stays available. No wait deadline or
periodic sleep determines success. The OCR CLI receives an ordinary Windows model
directory only after canonical identity comparison with the verified package path.
For mixed-language OCR, selected specialist models keep their configured order
and English is used last. The actual order, confidence and image locations are
reported; recognition is still machine-derived, not guaranteed exact source text.
The extraction revision changes with this policy so prior recognition is rebuilt.
Its local Tesseract interface emits located OCR lines, geometry, confidence,
languages, warnings and exact source/version identity. It performs no network
recognition or model download. Consumers may use the contract without HTML or
Windows supervision features; MailCraft enables both.

The supported importers are the pinned core adapters. They expose their native
coverage descriptions and warnings; unsupported import content is not implied to
have been recovered. Native text, hidden editable text, cached spreadsheet values,
formula/Python source and rendered-page OCR remain distinguishable. Formula and
Python source are inspected as text and are never run. Linked resources, macros,
scripts, plug-ins and external documents are not executed or loaded.

MailCraft maintains a derived SQLite FTS5 sidecar next to its native mailbox.
Metadata and previews can be listed without opening full bodies. Retained source
identities, extraction revision, edit/delete reconciliation and located evidence
prevent a prior document's hits from being applied to its replacement. Unknown
files at the sidecar name are preserved and reported, rather than overwritten.

Preparation and validation are separate:

1. `prepare-workers.py` reads verified archives and prepares independent donor
   workspaces in an explicitly registered D: compiler root. No build runs.
2. `resolve-ocr-sources.py` records an exact official stable source release and
   independent Git-object identities for the selected language models.
3. `prepare-ocr.py` prepares an isolated release-only vcpkg overlay. The installed
   vcpkg checkout and Pathweft cache remain unchanged.
4. At the complete feature/stage boundary, `build-workers.ps1` and `build-ocr.ps1`
   compile serially with one compiler job and record exact inputs and outputs.
   Donor lock verification compares resolved package/dependency identities.
   OCR selects an installed C++-qualified Visual Studio instance, initializes its
   exact x64 toolset in a child with a short tool PATH, and uses vcpkg's supported
   external environment triplet with ABI-tracked compiler/include/library paths.
   The parent environment and shared tool installations remain unchanged.
5. `assemble-runtime.py` assembles `craft-content/runtime.json`, all seven workers,
   local OCR/models, provenance and licences beside the product executables.
   Packaging is not qualification; the actual package still needs native,
   rendering, offline/restart and live-app checks through the installed gate.

No preparation/build script launches a GUI, starts an MCP service, alters the
selected application, changes Coordinator, or publishes a release. Register exact
scratch before use and clean expendable native/compiler trees through the installed
transactional-cleanup workflow after their final consumer exits. Retain the usable
package, compact receipts, selected source pins and required recovery material.

For Pathweft reuse, consume the versioned Rust contract and pinned adapter source;
do not depend on a running MailCraft process, its mailbox or private SQLite schema.
The protocol keeps its native importer/render/extraction boundaries explicit.

The current Windows runtime selects these importers; an extension identifies an
import route, not a promise that every object in that format can be recovered:

| Core | Extensions |
| --- | --- |
| Word | `docx`, `docm`, `doc`, `odt` |
| Grid | `xlsx`, `xlsm`, `xls`, `xlsb`, `ods` |
| Deck | `pptx` |
| Pdf | `pdf` |
| Photo | `pcraft`, `psd`, `psb` |
| Vector | `vectorcraft`, `drawcraft`, `svg`, `svgz`, `ai` |
| Fig | `figcraft`, `fig` |

The packaged OCR models are English (`eng`), Spanish (`spa`) and Greek (`ell`).
Other scripts may have native text extracted without OCR recognition support.
Image recognition and page rendering retain their own warnings; imported font
coverage can differ from native text coverage. For example, an absent rendered
glyph does not remove its original Unicode character from an editable text record.
Encrypted input, unsupported embedded objects and resource limits are explicit
gaps, never an empty successful document.

`protocol::Request` uses version1 and an echoed integer id. A consumer opens an
independently hashed input in its exact scratch directory, reads `info`, requests
`extract` with the original `{unit, offset}` cursor until `next` is absent, optionally
requests a bounded `render`, then closes and observes the whole native tree exit.
`Record.offset` is a UTF-8 byte offset inside its located text unit. `Derivation`
distinguishes native text from OCR engine/version/languages; OCR locations include
image geometry and confidence. The contract caps input at 64 MiB, decoded data at
256 MiB, images at 64 million pixels and each serialized text batch at 48 KiB. Importer-
specific limits and coverage remain in each returned report. These are explicit
exclusions, not an arbitrary mailbox or document-count limit.

Reuse requires the contract crate, the exact pinned adapter/build inputs and
the manifest-bound native payload with its licences. The finite native acceptance
fixture (`apps/mailcraft/src/headless/index_native_tests.rs`) exercises all seven
cores, located text, cached spreadsheet values/formula source, mixed-language OCR
and offline reopen through authenticated MCP; the app-specific part is the test
consumer, not a dependency required by another host. Existing Pathweft rendering
remains independent; adopting this contract there requires its own integration
and acceptance, without starting another MailCraft instance.
