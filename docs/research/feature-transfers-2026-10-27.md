# Feature transfers from the 99-repository cohort

This ledger follows the [99-repository comparison](99-file-projects-2026-10-27.md).
These are independently implemented adaptations to Commander's existing core,
not a claim that all upstream functionality was ported. No BSL Cmdr code or
GPL application implementation was copied into Commander.

## Implemented in this batch

| Inspiration / inspected source | Concrete gap before | Added behavior |
| --- | --- | --- |
| [Kiorg tab model](https://github.com/houqp/kiorg/blob/main/crates/kiorg/src/models/tab.rs), [Kiorg features](https://github.com/houqp/kiorg#key-features), [Yazi](https://github.com/sxyazi/yazi) | Only one live directory state in each pane | Independent folder tabs in both panes; per-tab selection, cursor, marks, history, filters and view state during the session; folder/view restoration across restart |
| [Kiorg previews](https://github.com/houqp/kiorg#key-features), [Yazi previews](https://github.com/sxyazi/yazi#features) | Source displayed as plain wrapped text | Basic Rust/C/C++/Python/TOML highlighting using the existing egui-extras lexer; optional horizontal scrolling via Wrap lines toggle |
| [QtFM custom commands](https://github.com/rodlie/qtfm), [lf external-command workflow](https://github.com/gokcehan/lf), [Strata custom actions](https://github.com/lgse/strata/blob/main/docs/custom-actions.md) | Command templates existed, but useful starters, names and extension filters required manual setup or JSON editing | Five reviewed command starters; editable template labels and extension filters; explicit Save/update keeps the same command in its original row |

### Folder tabs

- `Cmd+T`: open a new tab at the current folder.
- `Cmd+Shift+W`: close the active tab, retaining at least one per pane.
- `Cmd+Shift+[` / `Cmd+Shift+]`: previous/next tab, wrapping at the ends.
- Palette: **Open highlighted folder in new tab**.
- Each pane has an eight-tab cap. Plus/close buttons and a horizontally scrolling
  strip expose the same operations; names expose full paths on hover and to AT.
- Inactive panels drop filesystem watchers and cancel listing/size work.
  Activation retires stale rows before the app wires the shared workload lane
  and reads a fresh listing. Deleted or changed background files cannot remain
  actionable merely because an old tab remembered them.
- Opening/switching/closing a tab clears cross-pane previews, drag targets,
  comparison cache and pending context menus. Pending modal requests and active
  delete operations prevent core tab changes. Existing transfers retain their
  captured source/destination; they are not retargeted by tabs.
- Swapping panes swaps the complete tab groups, including their active indices.
- Session persistence stores folders, active indices and view configurations.
  History, selections, filters and marks remain session-lifetime. Invalid saved
  rows are skipped individually; inactive folders are restored without probing
  remote mounts. The already resolved current folder still wins over a saved
  record, including explicit launch paths.

### Source previews

- The existing cancellable, bounded text loader remains the owner of file I/O.
  Formatting is cached against text allocation, wrap width, scale, palette,
  language and document mode; the full text is not re-highlighted each frame.
- Coloring uses the basic lexer already included with egui-extras, not a full
  language parser or syntax engine. Unsupported extensions stay plain text.
- Syntax colors are limited to 64 KiB. Larger loaded text stays visible without
  coloring; Increase Contrast also disables syntax colors. The wrap preference
  is remembered per pane for the running session.
- Source remains selectable and unchanged; turning off wrapping enables the
  existing horizontal scroll area. Markdown/HTML Formatted/Source remains intact.

### Command starters and template metadata

The command bar's **Command recipes** menu fills the editable command line; it
never launches the chosen recipe by itself. The normal Run action, captured
opening selection, quoting, trust gate, process cancellation and bounded output
handling are reused.

| Starter | Required external program |
| --- | --- |
| SHA-256 report | `shasum` |
| Identify file formats | `file` |
| Count lines | `wc` |
| Show disk usage | `du` |
| Inspect media metadata | `ffprobe`; choose one media file |

No external tool is installed. Enter names and comma-separated extensions under
**Template name and file types**. Empty input or `*` means all files; e.g.
`jpg, png, tar.gz` narrows when the saved template is offered. This is a template
visibility filter, not a rewrite of the selection passed to an explicit command.
Invalid filters disable saving instead of silently broadening the filter.
Updating an existing command's label/filter preserves ordering and avoids
creating a duplicate row. Save failures remain visible through existing toasts.

## Verification

- Full serial regression after tabs/source-preview integration: **2200 passed,
  3 intentionally ignored**, no failures.
- Later template changes and final safety checks were validated with scoped
  command, template, recipe, tab, preview and native-harness runs. Exact counts,
  logs and screenshots are in [verification evidence](../qualification/repository-features-2026-10-27/README.md).
- Native folder-tabs capture now requires three tabs with the third active;
  source-preview capture waits for actual loaded text. Both cases were added to
  the existing CI matrix. Merely showing a tab strip or a loading spinner does
  not satisfy the scene readiness condition.
- GUI captures use temporary fixtures and recording fake native effects. They
  do not establish complete VoiceOver navigation, installed-release quality or
  performance on large real-world datasets.

## Remaining comparative work

Miller columns, complete document/media viewers, URI-native network browsing,
provider installation, signed self-updates and full custom-action manifests
remain open. See the [Strata ledger](../strata-parity.md). The other cohort
repositories remain research inputs, not completed port milestones. A feature
will be promoted here only after an actual Commander implementation and relevant
verification; no performance advantage is inferred from an upstream README.


## Next transfer: CSV and TSV tables

Inspired by [Strata's delimited-table service](https://github.com/lgse/strata/blob/ac5ab4f22d86798a40d7ad30eb3528d56eecf137/src/services/table.rs),
Commander now offers **Table / Source** modes for `.csv` and `.tsv` text previews.
This is an independent egui implementation using the CSV parser crate, not copied
GTK rendering code. It preserves quoted delimiters, embedded newlines, escaped
quotes, Unicode, empty fields and uneven row lengths. There is no inferred header:
the first record remains row 1. Formula-like content stays literal text.

The existing asynchronous text loader retains its 256 KiB file limit. Table
parsing is cached per pane and content allocation, bounded to 500 records and
64 columns. Additional records/columns show a partial-preview notice. Scrolling
renders only visible rows; cells have fixed width and full-value hover text.
Switching content resets the table scroll. Source retains the loaded input and
existing line-wrap control. No spreadsheet formulas, workbook formats, table
editing, sorting or automatic semicolon detection are included.

Verification is recorded in
[table preview evidence](../qualification/table-preview-2026-10-27/README.md).

## Next transfer: organize folder tabs

The inspected [Double Commander main form](https://github.com/doublecmd/doublecmd/blob/master/src/fmain.pas)
exposes `actMoveTabLeft`, `actMoveTabRight` and `actCloseDuplicateTabs`.
Commander now independently implements these interaction ideas in Rust/egui;
no upstream Pascal code was copied.

Right-click any folder tab to move that tab left or right. The open directory
and active panel state remain bound to the same tab. The palette also offers
**Move folder tab left**, **Move folder tab right** and
**Close duplicate folder tabs**. Palette movement applies to the active tab.
Boundary moves and duplicate cleanup without duplicates are disabled.

Duplicate cleanup applies only to the pane containing the menu. It retains the
active instance of its path and the first instance of every other path, preserving
their history, selection, filters and view. It compares stored paths without disk
probes or canonicalizing symlinks. Closed duplicates now join the pane's bounded recently-closed history
and can be reopened with their runtime state; see the later restoration entry. Reordered tabs use the existing session
snapshot so their ordering survives restart. Pending operations/modal dialogs
block these mutations through the same workspace guard as existing tab commands.

[Verification](../qualification/tab-organization-2026-10-27/README.md)

## Next transfer: find inside text previews

[The Double Commander viewer](https://github.com/doublecmd/doublecmd/blob/master/src/fviewer.pas)
provides find-next/previous actions and a case-sensitive search option. Commander
now independently implements literal Unicode search inside its text preview.
The **Find in preview** field searches the text actually laid out on screen:
plain text, syntax-colored source, rendered Markdown/HTML, and CSV/TSV Source
mode. HTML scripts/styles hidden by the formatter are not indexed in formatted
mode. Table mode remains a separate renderer; switch to Source to search it.

**Prev / Next** and **Enter / Shift+Enter** while the field has focus wrap through
matches. **Match case** changes case sensitivity. A counter and an underline mark
the current match, and navigation scrolls it into view. Reflow keeps the current
match; a changed file, displayed content or query resets navigation. State is
separate per pane. The existing Cmd+F file-finder shortcut remains available.

This uses the existing regex library only to implement escaped literal Unicode
matching; user input is not a regex. Input is capped at 256 query characters and
4096 retained matches (a `+` after the count reports additional matches). The
existing 256 KiB loaded-text limit remains. Results are cached across unchanged
frames; no filesystem reads are introduced by search.

[Verification and native capture](../qualification/preview-find-2026-10-27/README.md)

## Next transfer: TAR browsing and searchable TAR contents

[ouch](https://github.com/ouch-org/ouch/blob/main/README.md) supports the same
list/decompress workflow across ZIP, TAR and compressed TAR; [mholt/archives](https://github.com/mholt/archives/blob/main/README.md)
provides stream-oriented, uniform access to archived files. Following those
interaction/API ideas, Commander now independently extends its existing archive
pipeline to plain `.tar`, and content search now visits regular text members of
both TAR and tar.gz/tgz instead of skipping TAR content altogether. No upstream
Go/Rust source was copied and no additional runtime tool was installed.

- Open TAR through normal archive activation or **Browse archive**, inspect/filter
  members and extract selected entries into the other panel. Extraction continues
  to use the existing system-tar adapter; normal-file no-clobber and literal-name
  handling are retained and tested for plain TAR.
- Recursive queries such as `path:inside.txt content:needle` now produce member
  hits from TAR and tar.gz. Results preserve member names, sizes, modification
  times, virtual archive-member locations and reveal-the-containing-archive behavior.
- Search reads regular files only: directory/link entries are not dereferenced;
  binary and oversized contents are skipped. Existing 1 MiB/member and 32 MiB/run
  text budgets and 100,000-member/run cap also apply to TAR. Each sequential TAR
  scan is additionally limited to 128 MiB of decoded stream, including payloads
  skipped while reaching later headers. Cancellation is checked between chunks
  of at most 64 KiB. This is a scan limit, not an extraction-size limit.
- The browser marks capped scans as partial; TAR per-member packed size is `n/a`
  because a compressed TAR stream does not report that metadata per member.
  Search provider labels now say archives instead of ZIP.
- Native verification exposed clipped extraction controls in the pre-existing
  archive pane. The member list now reserves both footer rows and optional
  detail/empty-status lines instead of a fixed 38 pt allowance.

Other compression formats, nested archive traversal, an archive VFS and writing
archives in place remain open; these projects' full feature sets have not been
ported. [Verification and native frame](../qualification/tar-content-2026-10-27/README.md)

## Next transfer: sortable archive contents

[Ark's archive part](https://github.com/KDE/ark/blob/master/part/part.cpp) uses an
archive sort/filter model and sortable columns. Commander now independently adds
sorting to its existing flat archive member browser:

- Choose Member, Size, Packed, Ratio or Modified in the Sort control, or click a
  column header. Clicking an active header reverses its primary key. New numeric
  and date keys start largest/newest first; Member starts ascending.
- Reverse and Folders first are separate controls; folders stay first even when
  the key is reversed. Name comparison uses lowercased paths, with deterministic
  path/member-index tie breaking. Ratio comparison uses exact u128 cross products
  rather than rounded displayed floats.
- ZIP supports all keys. TAR/tar.gz do not report per-member compressed sizes, so
  Packed and Ratio sorting are unavailable rather than sorting made-up values.
- A Modified column is shown when the pane has at least 560 logical points of
  available width; its sort option remains available in narrower panes.
- Only the row-index view is reordered. Listing positions, selected member
  identities, extraction mapping and cursor identity remain stable. Hidden
  selection counts use cached membership, replacing a binary search that would
  incorrectly assume a sorted list of original indexes after reordering.
- Filter and order changes rebuild/resort the view; unchanged frames retain it.
  Sorting is scoped to the current archive tool and is not saved between opens.

[Verification and native capture](../qualification/archive-order-2026-10-27/README.md)


## Next transfer: restore recently closed folder tabs

[Ranger's tab_restore action](https://github.com/ranger/ranger/blob/master/ranger/core/actions.py)
restores a tab from a stack of closed tabs. Commander now independently provides
**Reopen closed folder tab** in the palette and on **Cmd+Shift+T**, plus a **Closed**
menu in each pane's tab strip to choose a specific recent tab (newest first).
No upstream Python source was copied.

The retained object includes directory, navigation history, selection/cursor,
filters and view settings. Reopening activates it at its saved position, clamped
to the current group, and follows the existing lazy directory revalidation path:
old rows are cleared and the app reconnects workers/watchers before loading.
Closed-tab workers/watchers remain suspended. Missing/changed paths are handled
by the normal listing behavior rather than a synchronous probe in the shortcut.

Each pane retains at most eight closed panels during this run. History is not
serialized; open-tab session restoration is unchanged. Closing the final tab is
still prohibited. Reopening at the eight-open-tab cap or during a guarded modal
leaves the history intact. Closed history moves with its tab group on Swap Panels.
Closing duplicates also retains removed panels; when reopened LIFO without other
intervening reordering, interleaved duplicates reconstruct their original order.
User-selected older entries reopen at their recorded position in the current group.

[Verification and native capture](../qualification/reopen-tabs-2026-10-27/README.md)

## Next transfer: image preview pan and zoom

[Kiorg's image preview](https://github.com/houqp/kiorg/blob/main/crates/kiorg/src/ui/preview/image.rs)
provides image pan/zoom and double-click reset. Commander now independently adds
an interactive viewer to its existing bounded, color-managed image pipeline.

Fit resets position and magnification. Plus/minus and image-hover zoom gestures
change magnification between 25% and 800% **of Fit**, explicitly labeled as such.
Fit preserves aspect ratio and does not enlarge a smaller decoded bitmap by
itself. Drag the image or use ordinary scrolling to pan. Double-click resets to
Fit. Close/Esc remain available; clicking the image now interacts with the viewer
instead of closing it. View state is bounded to the last image per pane; changing
the path resets to Fit. Source files are not modified.

Larger magnification feeds a larger logical target into the existing asynchronous
cache, retaining its 4096-physical-pixel target and decoded-memory limits. The
existing smaller texture stays visible while a sharper decode loads. This does
not promise original-resolution pixels at all magnifications: the displayed
percentage is relative to Fit and source/cached detail remains bounded. Pan does
not create a second bitmap or change source orientation/color handling.

Verification also exposed an idle native-QA observation gap: the harness
previously scheduled its periodic repaint only when no capture was pending.
It now keeps that timeout/retry heartbeat alive while a screenshot is pending.
Its capture checks and timeout threshold were not relaxed. The CI matrix adds an
actual loaded PNG at 200% of Fit (ten native scenarios total).

[Verification and native frame](../qualification/image-zoom-2026-10-27/README.md)


## Next transfer: basic POSIX permission editor

[Double Commander's file-properties editor](https://github.com/doublecmd/doublecmd/blob/master/src/fsetfileproperties.pas)
provides octal permission entry and include/exclude masks. Commander independently
implements a smaller Unix-only editor, available as **Change permissions** in the
command palette. No upstream implementation code was copied.

Nine owner/group/other read/write/execute controls preserve untouched mixed bits;
three-digit octal entry replaces these nine bits. Existing special bits are
retained in the requested mode. A virtualized before/after preview precedes Apply.
Inspection and mutation run on the workload workers, with cancellation and
per-item results. The modal uses cached policy state without introducing a broad
command-context filesystem probe into its paint loop.

This operation is non-recursive: every explicitly selected object is retained,
including a selected child of a selected directory and selections in other folders.
Foreign selections receive an identity-bound worker preview; vanished selections
remain errors rather than silently disappearing. Descendants commit before selected
ancestors that might lose traversal access. Current-folder listing identities and
preview modes are checked again through protected, non-following file handles
before mutation; symlinks and special files are rejected. Hard-link aliases may
report one change and one already-unchanged result.

Scope is deliberately partial: no ownership, ACL, recursive editing, elevation,
special-bit controls, persistent journal or Undo. A readable protected handle is
required, so even an owner may be unable to edit mode-000/unreadable objects here.
There is no path-based chmod fallback. Removing owner-read is explicitly called
out in the UI. Cancellation keeps completed changes; a batch is not atomic.
Readback verifies the resulting mode and reports filesystem differences.

[Verification and native frame](../qualification/permissions-2026-10-27/README.md)


## Next transfer: source line numbers and Go to line

[Double Commander's viewer](https://github.com/doublecmd/doublecmd/blob/master/src/fviewer.pas)
exposes line numbers and a Go to line action. Commander independently adds these
to its existing bounded text/source preview without copying upstream code.

The **Line numbers** toggle and line count accompany a **Go to line** field/button;
Enter in that field also jumps. A line is a logical newline-delimited source line,
not a wrapped visual row. Numbers appear only at each logical line's start,
including empty and trailing lines. Invalid or out-of-range input stays put and
shows the valid range. Jumping is immediate and does not change the file cursor.

The gutter is painted separately from selectable text, so source and find offsets
remain unchanged. Syntax colors, high contrast, wrapping, and the Source view of
Markdown/HTML/CSV remain supported; formatted documents/tables do not show source
line controls. The toggle is kept per pane during the session. Line indexes reset
on new content; geometry is cached per galley and only visible numbers are painted.
This retains the existing bounded whole-text loader, not a streaming large-file
viewer or editor; the horizontal gutter scrolls with the source.

[Verification and native frame](../qualification/preview-lines-2026-10-27/README.md)


## Next transfer: navigate archive folders

[Ark's archive view](https://github.com/KDE/ark/blob/master/part/part.cpp) uses
folder/tree activation to navigate archives. Commander independently adds a
folder-oriented view to its existing ZIP/TAR/tar.gz browser, retaining **All
members** as the default flat/global-filter view. No upstream implementation is
copied and no full Ark parity is claimed.

**Folders** shows direct child folders and files. Implicit parent folders are
synthesized even when the archive contains only deep file entries. Click a folder
(or reach it with the filter's arrows and press Enter) to enter it. **Up**, archive
root and component breadcrumbs navigate back. Navigation folders stay first;
member sorting continues to apply to files. Filtering in this view is scoped to
the current folder's displayed rows; All members provides the existing global
archive filter. Rows remain virtualized and indexes are cached between repaints.

Selections retain their real listing IDs across folder changes, filtering and
sorting. The extract button counts selections outside the visible folder as
hidden. **Select visible files** never adds a synthetic navigation row;
**Select folder contents** adds all listed real members under the current path,
including subfolders and filtered-out entries. **Clear** still clears selection
across the archive. **Extract all** retains its whole-listed-archive meaning.

The derived folder index is built on the archive worker, without filesystem
probes during navigation. It preserves native path identities and distinct
member IDs even for duplicate names. Index limits are 100,000 unique folders,
16 MiB of aggregate unique-folder path bytes and 128 components per member.
Exceeding them disables Folder view with a reason and retains the flat listing;
these are indexing limits, not a total-process memory claim. The existing capped
listing/unsafe-member rules and extraction policy remain authoritative. Synthetic
folders do not enter the archive's real member collection or extraction IDs.
No archive modification, nested-archive opening or streaming uncapped listing is
implied.

[Verification and native frame](../qualification/archive-folders-2026-10-27/README.md)


## Next transfer: native PDF page preview

[Kiorg's ebook/PDF preview](https://github.com/houqp/kiorg/blob/main/crates/kiorg/src/ui/preview/ebook.rs)
exposes document imagery and page metadata; Strata's document-preview comparison
also identified PDF as a gap. Commander independently implements a macOS page
raster viewer rather than feeding PDF bytes into the text loader. No upstream
implementation code is copied.

The PDF marker follows the selected file through existing preview synchronization.
Previous/Next and direct page-number entry (button or Enter) request individual
pages. Fit and 25–400% zoom operate over a fixed raster; source dimensions, crop
and page rotation feed the native drawing transform. **Open externally** hands
the file to the existing opener port. The raster view does not execute links/scripts, fill forms, edit annotations, or
unlock passwords. The later Page text extension below adds per-page text search
and copying.
No universal PDF compatibility or complete viewer parity is claimed.

Each request opens a regular source file on a Preview workload worker, checks the
listing size/mtime before and after bounded reads, and gives Core Graphics a data
provider over that snapshot. Native document/page pointers remain on their one
worker and release before input storage. Closing/changing the preview cancels or
discards old results; textures are held only for the active document/page per pane.
Provider admission and the existing image-preview runtime/rollout controls apply.

Limits are 64 MiB source, a 2048-pixel raster long edge, up to 100,000 pages and two
in-flight native renders. A 30-second UI wait becomes an explicit retryable error.
Read cancellation is cooperative; Core Graphics calls cannot be preempted. A hung
native call can retain one of the two native slots until it returns. Source/raster
limits are not a total memory or CPU guarantee for native PDF internals. Zoom does
not decode additional detail. Non-macOS hosts display an unsupported message and
retain external opening.

The first native raster test exposed an upside-down, unscaled centered page; the
renderer now explicitly scales the native point-space transform and keeps the
bitmap coordinate direction. A separate QA pointer script initially dragged
through the mandatory background-hover probe between press/release. Its movements
are now isolated during the PDF click sequence, then the ordinary hover check is
restored before capture. No capture thresholds were relaxed.

[Verification, limits and native frame](../qualification/pdf-preview-2026-10-27/README.md)


## Next transfer: PDF text, find and copying

Continues the [Kiorg-inspired PDF preview](https://github.com/houqp/kiorg/blob/main/crates/kiorg/src/ui/preview/ebook.rs)
with independent native text extraction. **Page image / Page text** switches
between the raster and reflowed text of the current page. Text extraction runs
only when requested, using the existing bounded PDF snapshot reader and worker
admission/cancellation rules. It does not add PDFKit parsing to ordinary image
page requests.

Page text reuses the Unicode literal preview-find controls, match case and
previous/next navigation. **Copy page text** emits the complete extracted page
through egui's clipboard output; ordinary text remains selectable. Search is
within this page, not across the document. Page controls remain available and
image zoom is disabled in text mode. A missing text layer shows an explicit
empty-page message; this is not OCR or a guarantee of the PDF's visual reading
order. Copy-restricted documents are declined according to PDFKit's permission
flag; no password/unlock bypass is added.

Each PDFKit document owns a private NSData copy and stays on its worker with
its page/string objects and autorelease pool. UTF-8 is copied by exact byte
length, preserving embedded NULs instead of using C-string termination. Copied
page text is capped at 256 KiB; oversized pages report an error and retain the
image mode. This caps copied output, not native parsing allocations or CPU.
Only current-page text/image data and one text layout per pane are cached;
closing/changing the document removes text, layout and find state. File-size,
concurrency, non-preemptible native-call and UI-timeout limitations from the PDF
page viewer still apply. Source files are never modified.

[Verification and native frame](../qualification/pdf-text-2026-10-27/README.md)


## Next transfer: natural archive name order

[Ark's archive sort model](https://github.com/KDE/ark/blob/master/part/archivesortfiltermodel.cpp)
enables numeric collation for human-readable archive names. Commander now applies
its existing, independently implemented directory-name comparator to archive
members and synthetic navigation folders too. No Ark/Dolphin comparison code is
copied; the directory comparator was moved unchanged into a shared module.

**Natural names** is enabled by default for each newly opened archive. `file2`
precedes `file10`; turning it off restores lowercase string ordering. Leading
zeros are a final tie-breaker after the rest of the name. Digit runs do not need
to fit an integer, so very long numbers do not overflow. This is ASCII-digit
natural ordering over the existing lowercase display paths, not Qt locale
collation or an implementation of Ark's extension-specific comparison rules.

The same switch orders navigation folders and breaks equal size/packed/date/ratio
keys by ascending name regardless of primary direction, matching the directory
list's tie policy. Name reversal reverses the main name comparison. Original
paths, archive indexes and listing positions still provide deterministic ties;
no duplicate archive member is dropped. Sorting invalidates cached row order but
retains cursor/selection IDs, including members hidden by the current filter.
The checkbox is session-only for that open archive, consistent with the other
archive sort controls. Archive records and extraction IDs are not rewritten.

[Verification and native frame](../qualification/archive-natural-2026-10-27/README.md)


## Next transfer: saved batch-rename rules

[Double Commander's multi-rename presets](https://github.com/doublecmd/doublecmd/blob/master/src/fmultirename.pas)
provide named reusable rename settings. Commander independently adds presets to
its existing batch-rename studio; no upstream implementation code is copied.

**Save new preset**, **Update selected**, **Delete preset** and a saved-rule
selector retain find/replace, literal/regex mode, prefix/suffix, case and active
numbering settings. Loading a preset only fills the editor and rebuilds the
current live preview. File targets and paths are not part of a preset. The normal
rename action still owns planning, collision checks, ordering, rollback and Undo.
Preset controls and their name field do not trigger the editor's global Enter
shortcut, including when a single-line edit gives up focus on Enter.

Up to 32 rules are stored in the existing atomic session store and restored at
startup. Preset edits attempt a session save immediately using the existing store
gate; blocked, failed or non-durable writes are reported rather than silently
claiming persistence. In-memory changes are retained for recovery. Cancelling the
rename does not undo independently saved preset edits. Updating is explicit;
Save new refuses a duplicate lowercase/NFC name instead of overwriting it.

Names are trimmed/NFC-normalized and limited to 64 characters without controls.
Each rule text field is capped at 4096 UTF-8 bytes; regexes must compile and saved
counter values must fit the editor's ranges (start 0–100000, step 1–1000, pad 0–6).
Deserialization validates before a rule can enter the saved collection. Invalid
stored presets fail the existing session gate closed and leave source bytes
intact. Older sessions default to an empty collection. This is Commander's own
session format, not an importer for Double Commander preset files.

The sheet now has a viewport-bounded outer scroll area. Visual inspection also
caught narrow grid fields and missing marker glyphs: editable fields now have a
minimum width, and preview status markers use ASCII. Existing rename semantics
remain unchanged.

[Verification and native frame](../qualification/rename-presets-2026-10-27/README.md)


## Next transfer: SHA-256 calculation and expected comparison

Inspired by [Double Commander's checksum dialog](https://github.com/doublecmd/doublecmd/blob/master/src/fchecksumcalc.pas),
Commander now calculates SHA-256 alongside BLAKE3 in one fresh file read. Its
existing palette command gains expected SHA-256 comparison for a single file,
clear match/mismatch/invalid states, and copying a GNU-format checksum list of
successfully computed files. Full-path reports disambiguate equal basenames.
Cancellation checks now run inside large files; changed files and substituted
paths do not publish partial digests. The previous short content hash is retained.
The implementation uses RustCrypto sha2 rather than upstream application code.

The copy format preserves UTF-8 filenames and refuses lossy conversion. macOS
system-shasum compatibility is qualified for spaces, backslashes, linefeeds and
Unicode; its carriage-return escape limitation is explicitly recorded. Importing
or batch-verifying manifest files remains outside this transfer.

[Verification and native frame](../qualification/sha256-2026-10-27/README.md)


## Next transfer: image preview rotation and mirroring

[Double Commander's built-in viewer](https://doublecmd.github.io/doc/en/viewer.html)
provides left/right quarter turns and horizontal/vertical mirrors. Commander
independently adds these viewing controls beside its existing Fit/zoom tools,
with a separate **Reset orientation** button.

Actions compose in display space, so a horizontal mirror always flips what is
currently on screen, including a previously rotated image. Quarter turns swap
the displayed aspect ratio and the cache's requested decode axes. Orientation
changes reset scroll position while preserving zoom. Fit and double-click reset
zoom/position and preserve the chosen orientation. A different path resets the
view; the two panes keep independent state. The controls transform texture
coordinates without copying, resampling or saving image pixels. Existing decode
limits and image-preview gates remain in force. This is preview orientation,
not an image editor or an EXIF-orientation writer.

[Verification and native frame](../qualification/image-orientation-2026-10-27/README.md)


## Next transfer: BOM-marked Unicode text previews

[Double Commander's viewer](https://github.com/doublecmd/doublecmd/blob/master/components/viewer/viewercontrol.pas)
supports UTF-16/UTF-32 encodings. Commander independently adds automatic decoding
of BOM-marked UTF-16LE/BE and UTF-32LE/BE to its existing background preview
loader. Existing find, line numbers and text/document/table views receive UTF-8
content without rewriting source files.

Strict Unicode decoding rejects malformed sequences; existing binary checks and
256 KiB source/output limits apply. UTF-32 signatures are checked before the
shorter UTF-16 prefix. BOM-free legacy encoding detection and a manual encoding
menu remain open. This transfer does not change the filesystem search engine.

[Verification and native frame](../qualification/unicode-preview-2026-10-27/README.md)


## Next transfer: transparency backgrounds in image previews

[Double Commander's viewer](https://doublecmd.github.io/doc/en/viewer.html)
provides a checkerboard to reveal image transparency. Commander independently
adds that background plus White/Black choices for examining light and dark
artwork. The choice is separate from orientation, Fit, pan and zoom, resets on a
different source path and remains independent per pane. Image files and alpha
values are unchanged.

A shared 2×2 repeated texture and one clipped quad keep checkerboard geometry
bounded even for large zoom. Native QA now exercises background switching and
includes real zero/partial/full-alpha PNG regions.

[Verification and native frame](../qualification/image-transparency-2026-10-27/README.md)


## Next transfer: paged read-only hexadecimal viewer

[Double Commander's Show as Hex mode](https://doublecmd.github.io/doc/en/viewer.html)
provides offset/hex/character columns. Commander independently adds **View as
hex** to its palette, opening the cursor file without changing the selection.
It supports 4 KiB background page reads, First/Previous/Next/Last navigation,
decimal or hexadecimal byte offsets, Reload, Cancel read and Copy page.
Large regular files use direct seeking instead of whole-file loading; file
identity/version checks reject detected changes and stale completions.

This is an explicit read-only inspection window. Hex search is added in the next transfer below. Editing, decimal
byte display and linked byte/character selection remain unported. The regular
text preview pipeline is unchanged.

[Verification and native frame](../qualification/hex-preview-2026-10-27/README.md)


## Next transfer: streaming hexadecimal byte search

[Double Commander's viewer search](https://doublecmd.github.io/doc/en/viewer.html)
accepts hexadecimal sequences and searches in either direction. Commander
independently adds **Hex bytes**, **Find next** and **Find previous** to its Hex
preview. Searches cover the file from the chosen offset, retain cross-block
matching state, support overlaps and do not wrap. A match opens the corresponding
page and highlights its rows. Patterns are explicit hex, 1–256 bytes.

Background jobs use bounded 64 KiB reads, progress and cancellation, preserving
the viewer's file-version checks and rejecting stale results. A no-match scan
also validates the source before reporting success. No file editing, wildcard
or regex matching is included in this transfer.

[Verification and native frame](../qualification/hex-search-2026-10-27/README.md)


## Documentation correction: root README recovery

The root README was restored after a stale script variable replaced it with
visual-QA source text. Earlier uncommitted documentation was reconstructed and
checked against its prior recorded SHA-256; later feature descriptions and all
17 native-scenario commands are retained. Historical evidence hashes were not
rewritten. [Recovery evidence](../qualification/readme-recovery-2026-10-27/README.md).


## Next transfer: regular-expression search in text previews

[Double Commander's viewer](https://doublecmd.github.io/doc/en/viewer.html) offers
regex search. Commander adds an optional Regex mode to its shared preview-find
control, retaining literal search as the default. Unicode ranges, zero-width
markers, navigation, displayed-document and current-PDF-page scope reuse the
existing selection/scroll behavior.

Regex compilation/indexing is debounced and scheduled in the background, with
Cancel, Retry, syntax errors and bounded input/program/result sizes. Old results
are discarded on query/text/path changes, and hidden preview jobs are cancelled
independently per pane. No replacement or file editing is introduced.

[Verification and native frame](../qualification/regex-preview-2026-10-27/README.md)


## Next transfer: choose the component used by batch find/replace

[Double Commander's Multi-Rename Tool](https://doublecmd.github.io/doc/en/multirename.html)
lets users control extension replacement. Commander independently adds Whole
name, Name stem and Extension scopes, with live preview and saved-preset support.
The default keeps previous whole-name behavior; legacy rules restore that same
default. Extension matching excludes the dot and skips folder find/replace, while
other configured stem edits retain their existing order.

Scope changes rebuild the validated plan before application. Existing captured
targets, collision checks, ordering, rollback and Undo/Redo are retained and
tested on temporary files. Native verification checks both a stem-only no-op
and the extension-change preview without applying either automatically.

[Verification and native frame](../qualification/rename-target-2026-10-27/README.md)


## Next transfer: first occurrence and ignore-case rename options

[Double Commander's Multi-Rename Tool](https://doublecmd.github.io/doc/en/multirename.html)
offers case matching and first-occurrence controls. Commander adds Ignore case
and First match only for both literal and regex replacement, composed with its
whole-name/stem/extension scopes. Defaults keep previous behavior; presets store
the options and the live cache rebuilds when either changes.

Literal replacement preserves dollar references as text even when its escaped
matcher uses Unicode case folding. Regex capture expansion retains standard
engine semantics. Actual rename, validation and Undo/Redo use the existing
transactional path, with temporary-file and native-preview verification.

[Verification and native frame](../qualification/rename-options-2026-10-27/README.md)


## Next transfer: Unicode capitalization in batch rename

[Double Commander’s case modifiers](https://doublecmd.github.io/doc/en/multirename.html)
inspire First capital and Title Words for name stems. Commander preserves the
last extension, saves these modes in presets and rebuilds the validated preview
on mode changes. Unicode word boundaries preserve separators; locale-specific
linguistic title casing is outside this implementation. Numbering has its own
row so the additional choices remain readable.

[Verification and native frame](../qualification/rename-capitalization-2026-10-27/README.md)


## Next transfer: whole-word search in previews

The [Krusader Handbook](https://docs.kde.org/trunk_kf6/en/krusader/krusader/krusader.pdf)
describes a Whole words search mode. Commander adds a Whole words checkbox to
its shared displayed-text search, including source, formatted documents and
current-page PDF text. Unicode regex boundaries compose with literal/regex
matching, existing navigation, cancellation and stale-result rejection. Native
verification toggles the real control and checks that three matches become two.

[Verification and native frame](../qualification/preview-whole-words-2026-10-27/README.md)


## Next transfer: explicit SHA-256/BLAKE3 comparison and digest copying

[Double Commander](https://doublecmd.github.io/doc/en/help.html) supports choosing
the algorithm when checking one file against an expected digest. Commander now
offers Compare as SHA-256/BLAKE3 and copies one selected digest. The shared
eligibility gate rejects cancelled, failed, skipped, malformed and ambiguous
reports; switching algorithms reevaluates the retained expected text immediately.
Native verification requires SHA-256 match → BLAKE3 mismatch → BLAKE3 match.

[Verification and native frame](../qualification/checksum-algorithms-2026-10-27/README.md)


## Next transfer: independent preview text size

[Double Commander’s viewer font controls](https://doublecmd.github.io/doc/en/configuration.html)
inspire A− / A+ / 100% in text previews. Commander scales text from 50–200%
independently of app zoom, with per-pane session settings and cache-aware reflow
for plain/source, syntax, formatted documents and PDF page text. Text ranges,
heading ratios, search and logical-line positions are preserved. Table view and
raster images keep separate behavior.

[Verification and native frame](../qualification/preview-text-size-2026-10-27/README.md)


## Next transfer: close other tabs and tabs to the right

[Explorer++](https://explorerplusplus.readthedocs.io/en/latest/panes/tab_bar.html)
provides Close Other Tabs relative to the context-menu target;
[Double Commander](https://doublecmd.github.io/doc/en/cmds.html) also offers
group tab closing. Commander adds Close other folder tabs and Close folder tabs
to the right. The chosen pane and anchor are explicit, surviving tabs retain
their state, and removed panels enter the existing bounded Closed history.
Native pointer input exercises both commands on inactive menu anchors.

[Verification and native frame](../qualification/tab-group-close-2026-10-27/README.md)


## Next transfer: open highlighted folders in background tabs

[Double Commander](https://doublecmd.github.io/doc/en/help.html) opens a folder
in a new tab without switching away. Commander adds an explicit background-tab
command in the palette and + context menu while retaining foreground commands.
The new tab inherits view settings and loads lazily; the current listing, cursor,
selection and preview remain live. Existing capacity and modal guards apply.

[Verification and native frame](../qualification/background-folder-tab-2026-10-27/README.md)


## Next transfer: selection groups and extension subtraction

[Double Commander](https://doublecmd.github.io/doc/en/help.html) offers
same-extension unselection and [attribute-based marking](https://doublecmd.github.io/doc/en/configuration.html).
Commander adds visible-file addition, visible-folder addition and same-extension
subtraction as explicit palette commands. They compose with existing selection,
respect the current filtered view and leave other-pane/filtered-out picks intact.
Native icon-grid verification excludes the PNG while retaining the folder and
other files, after waiting for actual asynchronous cursor publication.

[Verification and native frame](../qualification/selection-groups-2026-10-27/README.md)


## Next transfer: literal filename-list selection

[Double Commander](https://doublecmd.github.io/doc/en/help.html) can load selection
from a clipboard list. Commander adds Exact names (one per line) to Cmd+G: users
paste literal basenames and preview new selections before applying. Matching is
case-sensitive, additive and restricted to the filtered view. Invalid/oversized
lists are rejected as a whole. Enter edits the list; Cmd+Enter applies it.
The native name_list case expands the verification matrix to 21 scenarios.

[Verification and native frame](../qualification/name-list-selection-2026-10-27/README.md)


## Next transfer: file/folder target for group selection

[Double Commander](https://doublecmd.github.io/doc/en/configuration.html) offers
an independent attribute filter for group marking. Commander adds Apply to:
Files and folders / Files / Folders in Cmd+G, for masks and exact-name lists.
Target filtering applies to both addition and subtraction and shares the live
preview's cache key, preserving selections of other kinds and filtered-out rows.
Native pointer input checks counts changing 3 → 2 → 1 without applying selection.

[Verification and native frame](../qualification/selection-target-2026-10-27/README.md)


## Next transfer: copy all folder tabs to the opposite pane

[Double Commander](https://doublecmd.github.io/doc/en/cmds.html) provides
cm_CopyAllTabsToOpposite. Commander adds a palette/context-menu command that
copies ordered paths and view settings beside the destination's active tab,
keeping both current folders open. New panels load lazily; capacity is checked
for the entire group with no partial insertion. Native pointer input copies the
whole group from an inactive tab's menu and verifies both panes' resulting tabs.

[Verification and native frame](../qualification/copy-tab-group-2026-10-27/README.md)


## Next transfer: formatted JSON preview

[ranger’s scope.sh](https://github.com/ranger/ranger/blob/master/ranger/data/scope.sh)
offers JSON formatting. Commander adds native bounded Formatted/Source JSON
preview without subprocesses, preserving non-whitespace tokens, duplicate keys,
field order, numeric lexemes and string escapes. Invalid or oversized loaded
text falls back to source with an explanation. Existing find, wrapping, logical
lines and text-size controls work on the chosen view. The new json_preview
native case expands CI/README to 22 scenarios.

[Verification and native frame](../qualification/json-preview-2026-10-27/README.md)


## Next transfer: copy displayed preview text

[Double Commander’s viewer](https://doublecmd.github.io/doc/en/viewer.html)
supports copying viewed text. Commander adds Copy displayed text to ordinary
text previews, complementing its PDF control. The current galley is copied as
a whole through the existing ClipboardPort, so rendered/source modes produce
the representation the user sees. Native pointer input verifies exactly one
recording clipboard call with a payload equal to formatted JSON; other native
scenarios retain the zero-call requirement.

[Verification and native frame](../qualification/copy-preview-text-2026-10-27/README.md)


## Next transfer: optional enlargement of small image previews

[Double Commander’s viewer](https://doublecmd.github.io/doc/en/viewer.html)
provides Stretch and Stretch only large. Commander adds Enlarge small images,
default off, allowing aspect-preserving Fit to enlarge a small decoded texture.
The choice composes with relative zoom, orientation and transparency background,
remains pane-local and resets for a different file. Native pointer input enables
it on a 96×72 image while preserving the existing image-view checks.

[Verification and native frame](../qualification/image-enlarge-2026-10-27/README.md)


## Next transfer: independent extension case in batch rename

Inspired by [Double Commander’s multi-rename tools](https://doublecmd.github.io/doc/en/multirename.html), Commander adds Keep / lower / UPPER for the resulting last extension independently from stem case. This is an independent adaptation, not a claim of exact upstream UI parity. Presets preserve the choice, legacy rules default to Keep, and existing collision checks and Undo/Redo cover case-only changes.

[Verification and native frame](../qualification/rename-extension-case-2026-10-27/README.md)


## Next transfer: signed and descending rename numbering

[Double Commander’s counter](https://doublecmd.github.io/doc/en/multirename.html) accepts negative numbers. Commander now supports negative start/step and sequences crossing zero, sign-aware padding, a constant zero-step mode and signed preset round trips. Calculation widens before arithmetic; existing collision planning and Undo/Redo are retained. A real three-file regression verifies the descending sequence and unchanged payloads.

[Verification and native frame](../qualification/signed-numbering-2026-10-27/README.md)


## Next transfer: open the highlighted folder in the opposite pane

[Double Commander’s TransferLeft / TransferRight](https://doublecmd.github.io/doc/en/cmds.html) open the cursor directory in a specified pane. Commander adds a relative opposite-pane command to its palette and folder-tab menus. Source focus/view/selection remain intact; the destination closes preview and retains normal Back/Forward history. Full tab groups are supported without inserting tabs. The native folder_tabs scenario now verifies the menu action and resulting listing.

[Verification and native frame](../qualification/open-folder-other-pane-2026-10-27/README.md)


## Next transfer: home and filesystem-root navigation

[Double Commander’s Home / Root commands](https://doublecmd.github.io/doc/en/cmds.html) inspired two palette entries. Cmd+Shift+H opens the user's home; root navigation resolves the current path's filesystem/drive root. Both retain normal Back history, opposite-pane state and inactive tabs, and are guarded during confirmation. Plain Cmd+H keeps its hidden-files binding. Native folder_tabs now visits Home through keyboard input, then Root and Back before capture.

[Verification and native frame](../qualification/home-root-navigation-2026-10-27/README.md)


## Next transfer: image preview alignment

[Double Commander’s Center option](https://doublecmd.github.io/doc/en/viewer.html) inspired a Center image checkbox, on by default. Disabling it anchors smaller images to the canvas’s upper-left without changing size, zoom, orientation or transparency background. Changing alignment resets pan; pane/path state stays independent. A pointer-driven mesh test verifies actual movement without resizing, and native image_preview clicks the new control.

[Verification and native frame](../qualification/image-alignment-2026-10-27/README.md)


## Next transfer: decimal byte representation

[Double Commander’s Show as Dec](https://doublecmd.github.io/doc/en/viewer.html) inspired a HEX / Decimal switch in the paged binary viewer. Decimal byte values retain absolute hexadecimal offsets and the ASCII column; byte search remains hex-input. Copy page uses the chosen representation, and switching reuses the loaded page without another read. New tests cover all 256 values and pointer-driven copying; native hex_preview selects Decimal on its final partial page.

[Verification and native frame](../qualification/decimal-byte-preview-2026-10-27/README.md)


## Next transfer: choose batch-rename counter position

[Double Commander’s counter placeholder](https://doublecmd.github.io/doc/en/multirename.html) can appear within a composed mask. Commander now provides explicit Before name / After name placement around the decorated stem, preserving the final extension. Old rules default to After name; presets retain the choice. Signed numbering, cached planning and Undo/Redo compose with both positions. Native rename_options clicks Before name and checks the real resulting preview. Full mask/template parity remains open.

[Verification and native frame](../qualification/rename-number-position-2026-10-27/README.md)


## Next transfer: copy names without their last extension

[Yazi’s copy name_without_ext](https://yazi-rs.github.io/docs/configuration/keymap/) inspired a new palette command using Commander’s existing visible selection/cursor clipboard route. Lexical stems retain compound-name prefixes and dotfiles, preserve duplicate stems and escape embedded line controls. Focused tests cover formatting and selection routing; a full-run fixed-command dispatch failure was corrected and rechecked directly. No new native clipboard qualification is claimed.

[Verification and retained rejected evidence](../qualification/copy-name-stem-2026-10-27/README.md)


## Next transfer: visible whitespace in text previews

[Double Commander’s special-character display](https://doublecmd.github.io/doc/en/configuration.html) inspired a Whitespace toggle in plain/source and JSON previews. Dots and arrows are painted over original space/tab glyphs without changing copy/search text or layout. Per-pane state defaults off; markers respect the viewport. Native code_preview now enables the control on mixed indentation at 125% text size and retains existing search/line checks.

[Verification and native frame](../qualification/preview-whitespace-2026-10-27/README.md)


## Next transfer: F3 / Shift+F3 in focused preview search

[Double Commander’s viewer shortcuts](https://doublecmd.github.io/doc/en/shortcuts.html) inspired F3 / Shift+F3 next/previous-match navigation while the preview query field is focused. Existing Enter navigation and application F3 behavior outside that field remain intact. An input test covers cyclic navigation and focus gating; native code_preview focuses the query and waits for both actual match-index transitions before capture.

[Verification and native frame](../qualification/preview-find-f3-2026-10-27/README.md)


## Next transfer: optional wraparound in preview search

[lf’s wrapscan](https://github.com/gokcehan/lf/blob/master/doc.md) inspired a Wrap search preference over displayed-text matches. It defaults on; disabling it preserves the current result and stops at the first/last collected match, with shared keyboard/button boundary checks. It does not reindex or cancel search. Native code_preview clicks the control off and checks actual Prev/Next availability at the first match.

[Verification and native frame](../qualification/preview-search-wrap-2026-10-27/README.md)


## Next transfer: smart case in preview search

[lf’s smartcase](https://github.com/gokcehan/lf/blob/master/doc.md) inspired an optional Smart case checkbox. Unicode uppercase query characters enable case-sensitive matching, while manual Match case has priority. Literal and background regex search share the setting; regex syntax participates and inline flags retain precedence. Defaults remain unchanged. Tests cover Unicode and stale-job cancellation; native code_preview distinguishes Commander from commander with manual case unchecked.

[Verification and native frame](../qualification/preview-smart-case-2026-10-27/README.md)


## Next transfer: close a pointed folder tab

[Krusader’s middle-button tab close](https://docs.kde.org/trunk_kf6/en/krusader/krusader/folder-tabs.html) inspired direct middle-click and a Close this folder tab context action. Inactive slots close without activating them, disturbing the live preview or stealing pane focus. Closed state remains recoverable; the last tab and modal guards are retained. A native focus-stealing failure was corrected by letting tab actions own activation before body hit-testing; final tab/app/native checks passed.

[Verification and native frame](../qualification/middle-close-tab-2026-10-27/README.md)


## Next transfer: duplicate a pointed folder tab in the background

[Krusader’s Duplicate Current Tab](https://docs.kde.org/trunk_kf6/en/krusader/krusader/folder-tabs.html) inspired a context action that copies the pointed tab’s directory/view immediately beside it, including inactive tabs. The fresh panel is lazy and does not clone transient selection/filter/history. Live pane state/focus and Closed history remain intact; capacity/modal guards prevent partial insertion. Native folder_tabs duplicates an inactive folder distinct from the visible one and verifies the resulting group.

[Verification and native frame](../qualification/duplicate-pointed-tab-2026-10-27/README.md)


## Next transfer: move a folder tab to the other pane

[Krusader’s cross-panel tab movement](https://docs.kde.org/trunk_kf6/en/krusader/krusader/folder-tabs.html) inspired a context action transferring the owned panel into the destination’s background group. Selection, filter, view and directory history move with it; inactive transfers preserve current folders and focus. Last-tab, capacity and modal guards are atomic. Runtime tests activate the moved panel and navigate its preserved history; native folder_tabs verifies an inactive right-to-left transfer. A misplaced guard assertion from the broad run was corrected and all 27 tab tests rechecked.

[Verification and native frame](../qualification/move-tab-other-pane-2026-10-27/README.md)


## Next transfer: reset the current batch-rename rule

[Double Commander’s Reset all](https://doublecmd.github.io/doc/en/shortcuts.html) inspired Reset rule. It clears current edits, resets even inactive counter controls, detaches the preset and discards stale validation/error while retaining the original target context and saved presets. Native rename_options clicks it after prior transformations and requires a rebuilt unchanged-name plan with Rename disabled.

[Verification and native frame](../qualification/rename-reset-rule-2026-10-27/README.md)

## Next transfer: counter-only batch rename

[Double Commander’s counter-only name mask](https://doublecmd.github.io/doc/en/multirename.html) inspired Counter only: replace the decorated stem with a formatted counter while retaining the extension. Named presets retain this mode, existing collision validation applies, and disabling numbering restores ordinary naming.

[Verification and native frame](../qualification/rename-counter-only-2026-10-27/README.md)

## Next transfer: trim boundary whitespace in batch rename

[Czkawka bad-name cleanup](https://github.com/qarmin/czkawka/blob/master/instructions/Instruction_CLI.md) inspired Trim spaces: remove Unicode whitespace at the ends of the final name, retain internal spaces, save the option in presets and validate empty/colliding results.

[Verification and native frame](../qualification/rename-trim-spaces-2026-10-27/README.md)

## Next transfer: collapse repeated separators

[Czkawka duplicated non-alphanumeric cleanup](https://github.com/qarmin/czkawka/blob/master/instructions/Instruction_CLI.md) inspired Collapse repeats. Identical consecutive separator/symbol graphemes become one; repeated letters/digits and different separators remain. Presets, reset, preview and collision guards include the option.

[Verification and native frame](../qualification/rename-collapse-repeats-2026-10-27/README.md)

## Next transfer: clear preview search

[Yazi cancellation actions](https://yazi-rs.github.io/docs/configuration/keymap/#escape) inspired Clear beside the preview query: cancel pending regex work, remove query/matches/errors, retain options and return input focus. Native code_preview now clicks Clear after proving smart-case matching.

[Verification and native frame](../qualification/preview-clear-search-2026-10-27/README.md)

## Next transfer: image preview half-turn

[Double Commander’s +180 viewer action](https://doublecmd.github.io/doc/en/viewer.html) inspired Rotate 180° in Commander image preview. Display-space composition preserves mirrored orientations, zoom and appearance options; source files remain unchanged.

[Verification and native frame](../qualification/image-half-turn-2026-10-27/README.md)

## Next transfer: close folder tabs to the left

[Dolphin close-tabs-to-left/right actions](https://kde.org/announcements/changelogs/gear/26.08.0/) inspired the missing Close folder tabs to the left menu action. It retains the pointed tab and survivors, preserves recently-closed recovery and respects modal ownership. Native folder_tabs exercises the actual menu on an inactive tab.

[Verification and native frame](../qualification/close-tabs-left-2026-10-27/README.md)

## Next transfer: copy the pointed folder tab path

[Dolphin Copy Location](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/configuring-dolphin.html) inspired Copy this folder path in each tab’s context menu. It copies the exact pointed path, including background tabs, through the established formatter/clipboard feedback without activating the pane.

[Verification](../qualification/copy-pointed-tab-path-2026-10-27/README.md)

## Next transfer: Unicode normalization in batch rename

[rclone Unicode name transformations](https://github.com/rclone/rclone/blob/master/lib/transform/transform.md) inspired Keep/NFC/NFD/NFKC/NFKD choices for final names. Existing dependency, preset/reset support and post-transform validity/collision checks are retained. Filesystem representation enforcement remains outside the transform guarantee.

[Verification and native frame](../qualification/rename-unicode-normalization-2026-10-27/README.md)

## Next transfer: remove original name prefix and suffix

[rclone trimprefix/trimsuffix](https://github.com/rclone/rclone/blob/master/lib/transform/transform.md) inspired Remove prefix / Remove suffix fields. They remove one exact original boundary match before find/replace and extension splitting, with preset/reset/validation integration. Native rename_options types a removal prefix and proves the changed preview.

[Verification and native frame](../qualification/rename-remove-affixes-2026-10-27/README.md)

## Next transfer: limit the final name stem

[rclone extension-preserving truncation](https://github.com/rclone/rclone/blob/master/lib/transform/transform.md) inspired Limit stem: an optional final stem limit in complete Unicode graphemes, retaining the last extension. Includes presets/reset/cached preview and existing name/collision validation. Commander counts stem graphemes rather than matching rclone total-length semantics.

[Verification and native frame](../qualification/rename-stem-limit-2026-10-27/README.md)

## Next transfer: semicolon-separated selection masks

[Double Commander group-selection masks](https://doublecmd.github.io/doc/en/help.html) inspired semicolon delimiters alongside existing commas. Mixed masks share the established preview/apply parser and subtraction rules. Exact names continues to support literal delimiter characters.

[Verification](../qualification/semicolon-selection-masks-2026-10-27/README.md)

## Next transfer: case-sensitive selection masks

[Double Commander case-sensitive group selection](https://doublecmd.github.io/doc/en/cmds.html) inspired Match case in the mask dialog. Preserves original filename/extension case for globs, shorthand and exclusions, shares preview/application logic and invalidates the count cache when changed. Exact names remains independently case-sensitive.

[Verification and native frame](../qualification/case-sensitive-selection-masks-2026-10-28/README.md)

## Next transfer: ignore accents in selection masks

[Double Commander accent-insensitive group selection](https://doublecmd.github.io/doc/en/cmds.html) inspired Ignore accents. Comparison removes combining marks and expands common Latin ligatures on both sides; case sensitivity, exclusion priority, preview counts and Exact names isolation are retained. No filenames change.

[Verification and native frame](../qualification/ignore-accents-selection-masks-2026-10-28/README.md)

## Next transfer: include extensionless names with *.*

[Double Commander *.* marking compatibility](https://doublecmd.github.io/doc/en/configuration.html) inspired Treat *.* as * in the mask dialog. Exact *.* terms can include extensionless names, including exclusions, without changing other wildcard rules. Includes count-cache and case/accent/target integration.

[Verification and native frame](../qualification/extensionless-selection-mask-2026-10-28/README.md)

## Next transfer: select/deselect files sharing the cursor stem

[Double Commander current-name selection](https://doublecmd.github.io/doc/en/cmds.html) inspired command-palette actions to select or deselect visible files sharing the cursor’s name without its final extension. Case-insensitive comparison preserves unrelated/hidden picks and other-pane state; cursor-file capability predicates gate both actions.

[Verification](../qualification/same-stem-selection-2026-10-28/README.md)

## Next transfer: first/last visible file navigation

[Double Commander file-edge navigation](https://doublecmd.github.io/doc/en/cmds.html) inspired Go to first/last visible file in the palette. The commands skip directories in current display order, honor sorting/filtering, reveal the cursor and retain selections, other-pane state and the last repeatable action.

[Verification](../qualification/file-edge-navigation-2026-10-28/README.md)

## Next transfer: invert file/folder selection independently

[Double Commander attribute-filtered inversion](https://doublecmd.github.io/doc/en/cmds.html) inspired separate palette actions for visible files and visible folders. They preserve the other kind and filtered-out picks; per-kind availability and command dispatch are qualified.

[Verification](../qualification/kind-selection-inversion-2026-10-28/README.md)

## Next transfer: custom folder-tab titles

[Double Commander Rename Tab](https://doublecmd.github.io/doc/en/help.html) inspired editable custom titles in each tab’s context menu. Labels persist with tab session metadata and follow duplication, copying, moving and closed-tab recovery. Legacy/invalid metadata handling and popup input ownership are qualified; the native scenario names an inactive tab without changing pane focus.

[Verification and native frame](../qualification/custom-folder-tab-titles-2026-10-28/README.md)

## Next transfer: protect folder tabs from closing

[Double Commander locked-tab close exclusions](https://doublecmd.github.io/doc/en/cmds.html) inspired independent close protection. Direct/middle closing is blocked; groups and deduplication retain protected tabs. Protection follows tab lifecycle/session data while navigation remains available. Native folder_tabs enables protection on Work and verifies a middle-click cannot close it or switch pane focus.

[Verification and native frame](../qualification/protected-folder-tabs-2026-10-28/README.md)

## Next transfer: duplicate-tab retention priority

[Double Commander duplicate priorities](https://doublecmd.github.io/doc/en/help.html) inspired retaining protected copies, then custom titles, with active/first tie-breaking. Removed active copies remain recoverable; surviving panels retain their selection and filter. Native menu cleanup and regression coverage qualify the policy.

[Verification and native frame](../qualification/tab-deduplication-priority-2026-10-28/README.md)

## Next transfer: tab protection through the command palette

[Double Commander command-based tab options](https://doublecmd.github.io/doc/en/cmds.html) inspired Protect/Unprotect folder tab actions in the palette. State-sensitive availability and active-pane dispatch reuse persistent close protection while retaining navigation and runtime state.

[Verification](../qualification/tab-protection-palette-2026-10-28/README.md)

## Next transfer: first/last folder-tab navigation

[Double Commander indexed tab activation](https://doublecmd.github.io/doc/en/cmds.html) inspired first/last destination commands in the palette. They restore the chosen runtime panel in the focused pane, respect boundaries and modal guards, and preserve the last repeatable action.

[Verification](../qualification/tab-edge-navigation-2026-10-28/README.md)

## Next transfer: numbered folder-tab navigation

[Double Commander indexed tab activation](https://doublecmd.github.io/doc/en/cmds.html) inspired direct palette access to positions 1–8. Missing/current destinations are unavailable; runtime panels restore their state and modal guards prevent retargeting. Existing saved-folder digit shortcuts remain intact.

[Verification](../qualification/numbered-folder-tabs-2026-10-28/README.md)

## Next transfer: group tab closing from the palette

[Double Commander close-group commands](https://doublecmd.github.io/doc/en/cmds.html) inspired palette access to close others/left/right. Shared eligible-peer counts disable empty and all-protected groups in both palette and tab menus. Closed runtime states remain recoverable.

[Verification and native compatibility frame](../qualification/group-tab-palette-2026-10-28/README.md)

## Next transfer: selection memory

[Double Commander selection memory](https://doublecmd.github.io/doc/en/cmds.html) inspired Save selection in memory / Restore saved selection. Each runtime tab has one folder-bound buffer; restore replaces local picks using the complete listing and preserves other-folder picks. Empty, missing-entry, navigation, tab recovery and modal cases are qualified.

[Verification](../qualification/selection-memory-2026-10-28/README.md)

## Next transfer: sort file names without their final extension

[Double Commander name-without-extension sorting](https://doublecmd.github.io/doc/en/cmds.html) inspired a Stem sort column and palette action. Natural/plain order, folders-first, dotted directories, ascending full-name ties and current cursor/selection remain coherent; the header identifies the mode and view serialization retains it.

[Verification](../qualification/stem-name-sort-2026-10-28/README.md)

## Next transfer: case-sensitive name sorting

[Double Commander configurable case sensitivity](https://doublecmd.github.io/doc/en/configuration.html) inspired an optional original-Unicode-name sort policy. Full-name/stem keys and name ties respect it; natural numeric order stays independent. Pane/tab serialization and legacy defaults are qualified. This is code-point ordering, not locale collation.

[Verification](../qualification/case-sensitive-name-sort-2026-10-28/README.md)
