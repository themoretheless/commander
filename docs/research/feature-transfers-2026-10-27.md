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

Extended Miller-column interactions, complete document/media viewers, URI-native network browsing,
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

## Next transfer: independently name-ordered folder group

[Double Commander directory sorting](https://doublecmd.github.io/doc/en/configuration.html) inspired folders-first with independent ascending name order, while files retain their chosen column/direction. Grouping toggles remain consistent; pane/tab persistence and both-pane cursor/selection behavior are qualified.

[Verification](../qualification/folders-by-name-sort-2026-10-28/README.md)

## Next transfer: copy the current folder location from the palette

[Dolphin Copy Location](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/configuring-dolphin.html) inspired selection-independent current-folder copying from the palette, with an additional file-URL form using the existing formatter. Empty/filtered folders and dispatch-owner capture are qualified.

[Verification](../qualification/current-folder-copy-2026-10-28/README.md)

## Next transfer: deselect visible files/folders independently

[Double Commander attribute-filtered deselection](https://doublecmd.github.io/doc/en/configuration.html) inspired separate visible-file and visible-folder subtraction commands. Filtered-out/other-folder picks and the other kind survive; selected-visible-kind capabilities disable no-op palette entries.

[Verification](../qualification/kind-deselection-2026-10-28/README.md)

## Next transfer: explicit left/right pane focus

[Double Commander FocusSwap destinations](https://doublecmd.github.io/doc/en/cmds.html) inspired explicit Focus left/right pane palette actions. Destination availability, modal execution guards and preservation of tab/selection/filter/repeat state are qualified.

[Verification](../qualification/explicit-pane-focus-2026-10-28/README.md)

## Next transfer: pull the opposite pane folder into the focused pane

[Double Commander directional equal-folder navigation](https://doublecmd.github.io/doc/en/cmds.html) inspired the reverse companion to Equalize panels. Source/focus and destination Back/Forward state remain coherent; matching paths are no-op and both directions now reject modal retargeting.

[Verification](../qualification/pull-other-folder-2026-10-28/README.md)

## Next transfer: navigate selected visible entries

[fzf selected-item navigation](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1) inspired next/previous selected-entry palette commands. They respect current display order/filtering, preserve picks and repeat history, reveal destinations and stop at boundaries; directional capabilities are qualified.

[Verification](../qualification/selected-entry-navigation-2026-10-28/README.md)

## Next transfer: filtered clear and stationary selection toggle

[fzf selection actions](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1) inspired Clear visible selection and Toggle current selection in place. Clearing retains excluded/other-folder picks and selection memory; stationary toggling leaves Space behavior intact. Capability, parent-row and both-pane state are qualified.

[Verification](../qualification/filtered-selection-actions-2026-10-28/README.md)

## Next transfer: literal file-pane filter prefix

[fzf non-fuzzy quoted matching](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1) inspired an apostrophe-prefixed literal substring mode in pane filters. Shared matching/highlight ranges, significant spaces, facet/cache behavior and saved-query marker removal are qualified. This does not implement the full extended-search grammar.

[Verification](../qualification/literal-name-filter-2026-10-28/README.md)

## Next transfer: anchored prefix/suffix/whole-name filters

[fzf anchored matching](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1) inspired ^prefix, suffix$ and ^whole$ pane filters with literal text and shared highlights. Apostrophe escapes remain available; cache transitions and bounded saved-query conversion are qualified.

[Verification](../qualification/anchored-name-filter-2026-10-28/README.md)

## Follow-up: retain pane filter semantics when saving a query

The fzf-inspired filter transfer exposed a reproduced Unicode/export mismatch and loss of fuzzy subsequence semantics. New pane exports use the same matcher through a typed filter: predicate; legacy generic queries remain unchanged. Expression/JSON roundtrip and Unicode candidate comparisons are qualified.

[Reproduction and verification](../qualification/filter-export-semantics-2026-10-28/README.md)

## Next transfer: negated pane-name filters

[fzf negation](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1) inspired !text exclusions with optional prefix/suffix/whole-name boundaries. Shared matching/export semantics, absent-match highlights, cache transitions, facets and excluded-pick retention are qualified.

[Verification](../qualification/negated-name-filter-2026-10-28/README.md)

## Next transfer: OR alternatives in pane-name filters

[fzf OR filtering](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1) inspired standalone-pipe alternatives combining existing match forms. Highlight choice, partial-expression rejection, cache/facet behavior, excluded picks and saved-query backend agreement are qualified. Leading apostrophe keeps whole-input literal priority.

[Verification](../qualification/or-name-filter-2026-10-28/README.md)

## Follow-up: literal quoted branches inside OR filters

The fzf-inspired OR transfer now supports independently quoted literal alternatives, including a first branch containing a standalone pipe. This Commander extension preserves the original apostrophe whole-input form; malformed branch handling, cache transitions and saved-query agreement are qualified.

[Verification](../qualification/quoted-or-filter-2026-10-28/README.md)

## Next transfer: conjunctive pane-name filters

The fzf-inspired matcher now supports explicit && conditions with tighter precedence than OR, preserving existing fuzzy-space behavior. Quote-aware scanning, merged highlights, partial-expression rejection, facets and saved-query backend agreement are qualified. This is Commander syntax, not full fzf grammar parity.

[Verification](../qualification/and-name-filter-2026-10-28/README.md)

## Follow-up: current-folder selection clearing scope

Completes the fzf-inspired clearing actions with a current-folder scope, between visible-only and all-folder clearing. Filtered-out local picks are cleared while other-folder picks and saved selection memory survive; both-pane capability and dispatch behavior are qualified.

[Verification](../qualification/current-folder-selection-clear-2026-10-28/README.md)

## Extension query (2026-10-28)

Adapted fd extension filtering into Find: `ext:rs`, optional leading dot and compound suffixes, files only, case-insensitive across search modes; saved queries retain semantics. Repeated conditions use AND. Full suite 2473 passed / 3 ignored; all-target visual-qa Clippy passed. [Qualification](../qualification/extension-query-2026-10-28/README.md).

## Whole-name glob queries (2026-10-28)

Adapted fd whole-name mask search into Find: `glob:report-*.pdf`, quoted masks and Unicode `?`, independent of text mode; combines with metadata/content conditions and survives saved-query round trips. Commander masks support `*`/`?` with other characters literal, rather than full shell glob grammar. Full suite 2475 passed / 3 ignored; all-target visual-qa Clippy passed. [Qualification](../qualification/glob-name-query-2026-10-28/README.md).

## Search depth controls (2026-10-28)

Adapted fd minimum/maximum/exact depth controls into saved Find queries: `depth:<=1`, `depth:>=2`, `depth:2`, `depth:1..3`. Live traversal prunes at the maximum; indexed and archive member results obey the same relative levels, and boundary archives are not opened. Full suite 2477 passed / 3 ignored; all-target visual-qa Clippy passed. [Qualification](../qualification/search-depth-2026-10-28/README.md).

## Search name exclusions (2026-10-28)

Adapted fd exclusions into saved Find queries: `exclude:node_modules`, `exclude:*.bak`, quoted name masks and repeated exclusions. Live traversal prunes matching directory names; indexed paths and archive members obey ancestor exclusions, while the root is exempt. Excluded archive files are not opened. Full suite 2479 passed / 3 ignored; all-target visual-qa Clippy passed. Root-exemption defect and rejected runs retained. [Qualification](../qualification/search-exclusions-2026-10-28/README.md).

## File-only and exact-size search (2026-10-28)

Adapted fd file-type/exact-size controls: `type:file size:=0` finds empty non-directory entries; exact sizes support existing units and saved queries. Bare `size:1KB` retains minimum semantics. Integer byte parsing/multiplication now retains precision through u64::MAX and rejects overflow. Live/indexed/ZIP agreement checked. Full suite 2481 passed / 3 ignored; all-target visual-qa Clippy passed. [Qualification](../qualification/exact-size-files-2026-10-28/README.md).

## Optional archive search (2026-10-28)

Adapted Double Commander’s Search in archives option into saved Find queries: `archives:off` disables member expansion while retaining ordinary archive filename hits; `archives:on` and the default preserve expansion. Live/indexed behavior, disabled labels, invalid ZIP handling and query persistence checked. Full suite 2483 passed / 3 ignored; all-target visual-qa Clippy passed. [Qualification](../qualification/archive-search-scope-2026-10-28/README.md).

## Negative content search (2026-10-28)

Adapted Double Commander’s Find files NOT containing text: `notcontent:draft`, combinable with positive content conditions and saved queries in Exact/Fuzzy/Regex modes. Partial/binary/unreadable input cannot prove absence. New index records carry EOF completeness; legacy cached positives remain usable, while absence requires index refresh. Live/indexed/ZIP results and real read-cap limits checked. Full suite 2489 passed / 3 ignored; final focused mode checks and all-target visual-qa Clippy passed. [Qualification](../qualification/negative-content-search-2026-10-28/README.md).

## Case-sensitive Find queries (2026-10-28)

Adapted Double Commander case-sensitive search into saved Find queries: `case:sensitive` / `case:insensitive`. Applies across name/path/content, negative content, masks, extension suffixes, exclusions and pane-filter expressions in all modes. Streaming fuzzy alignment and original-coordinate highlights retain case policy. Live/indexed/ZIP matrix qualified. Full suite 2493 passed / 3 ignored; final eight focused tests and all-target visual-qa Clippy passed. [Qualification](../qualification/case-sensitive-search-2026-10-28/README.md).

## Hide and restore search results (2026-10-28)

Adapted Double Commander Remove from list / Show all found items: row hide and restore-all controls keep files and total match counts intact. Location keys survive batches and sorting, distinguish hard links/duplicate archive members, and filtered order drives navigation. New-search resets hiding. Full suite 2495 passed / 3 ignored; all-target visual-qa Clippy passed. Native click/layout qualification remains open. [Qualification](../qualification/hide-search-results-2026-10-28/README.md).

## Native qualification of result hiding/restoration (2026-10-28)

Added search_results to the native QA matrix (23 scenarios), real hide/restore/hide clicks, strict modal and framebuffer capture checks, and centered initial Search placement. Final bundled native capture passed 16/16 checks with zero recorded native port effects; frame visually inspected. Full visual-qa suite 2511 passed / 3 ignored; final QA unit checks 4 passed and all-target Clippy passed. Raw CLI first-frame stall remains an explicit runtime limitation; rejected evidence retained. [Qualification](../qualification/search-results-native-2026-10-28/README.md).

## Reveal Find results in background tabs (2026-10-28)

Adapted Double Commander Open In New Tab(s): per-result Tab button preserves Search and live pane, defers target focus until activation/listing, and reveals archive containers. Hidden targets enable hidden rows only in the new tab; common tab caps/modal guards apply. Both-pane regressions passed. Full visual-qa suite 2513 passed / 3 ignored; Clippy passed; bundled native clicks/cursor landing and 16 capture checks passed with zero recorded effects. [Qualification](../qualification/search-result-background-tabs-2026-10-28/README.md).

## Keyboard background tabs from Find (2026-10-28)

Completed keyboard access to the Double Commander-inspired result-tab action: query Up/Down + Cmd+Enter preserves Search/editor focus and skips hidden rows. Native testing exposed and fixed egui’s queued arrow focus move using the existing file-list cancellation pattern. Explicit editor identity and visible-order/archive target regression added. Full visual-qa suite 2514 passed / 3 ignored; Clippy passed; native query focus/new-tab capture 16/16 with zero recorded port effects. Rejected evidence retained. [Qualification](../qualification/find-keyboard-background-tabs-2026-10-28/README.md).

## Negative name/path text conditions (2026-10-28)

Adapted Double Commander-style negative text comparisons: notname: / notpath: use the selected mode/case, combine with other conditions and survive saved-query round trips. Relative paths exclude the root name; these filter results rather than prune traversal. Live/indexed/ZIP matrix and literal/fuzzy/regex distinctions qualified. Full visual-qa suite 2516 passed / 3 ignored; all-target Clippy passed. [Qualification](../qualification/negative-name-path-search-2026-10-28/README.md).

## Extension alternatives (2026-10-28)

Adapted fd multiple-extension filtering: `exts:rs,toml,tar.gz` combines alternatives with OR and other conditions with AND. Preserves literal single `ext:` and saved-query semantics; supports compound suffixes and case policy. Live/indexed/ZIP matrix and parser round trips passed. Full visual-qa suite 2518 passed / 3 ignored; all-target Clippy passed. [Qualification](../qualification/extension-alternatives-2026-10-28/README.md).

## Absolute modification-time bounds (2026-10-28)

Adapted fd absolute changed-after/before filters as `after:` / `before:`. Strict UTC date or offset-aware RFC 3339 bounds retain fractional precision and saved-query semantics. Live/indexed/ZIP matrix, pre-epoch/strict boundary/missing-time validation passed across all modes. Full visual-qa suite 2520 passed / 3 ignored; final all-target Clippy passed after doc-comment formatting correction. [Qualification](../qualification/absolute-modified-dates-2026-10-28/README.md).

## Precise modification-time index metadata (2026-10-28)

Closed a reproduced gap in the fd-inspired absolute-date transfer: indexed fractional timestamps were rounded and pre-epoch dates dropped. New snapshots preserve exact modification times with checked reconstruction; legacy snapshots remain readable at their original precision and require rebuilding for exact metadata. Live/indexed all-mode parity, persisted timestamp round trips, corrupt metadata and serialization budgets qualified. Full visual-qa suite 2522 passed / 3 ignored; Clippy passed. [Qualification](../qualification/precise-index-time-2026-10-28/README.md).

## Symbolic-link type search (2026-10-28)

Adapted fd link-type filtering as `type:symlink` / `type:link`, with Commander conjunction and saved-query semantics. Native/indexed/ZIP/TAR symbolic-link metadata qualifies broken/file/directory/loop links and excludes hard links. Legacy index kinds remain unknown and count as skipped until rebuild; cached kinds require no live stat. All-mode provider matrix, archive scope and JSON compatibility passed. Full visual-qa suite 2524 passed / 3 ignored; all-target Clippy passed. [Qualification](../qualification/symlink-query-2026-10-28/README.md).

## Executable-mode file search (2026-10-28)

Adapted fd executable filtering as `type:executable` / `type:exec`, explicitly defined as a lexical regular file with any Unix execute bit rather than effective-user access parity. Native/indexed/ZIP/TAR mode observations and saved queries supported; unknown old metadata counts as skipped until rebuild. All-mode matrix covers all execute classes, binary content, hard links, symbolic links, folders, FIFO, archive scope and cached permissions. Full visual-qa suite 2526 passed / 3 ignored; all-target Clippy passed. [Qualification](../qualification/executable-query-2026-10-28/README.md).

## Copy visible Find result paths (2026-10-28)

Adapted Nimble Commander Copy Item Path into Find: copies visible result paths in displayed/time-group order, skips hidden rows and retains virtual archive members. Uses existing clipboard formatting/feedback; empty lists disabled, streaming copies received rows and keeps query focus. Regression covers controls, hidden/restore/order/archive semantics. Full visual-qa suite 2527 passed / 3 ignored; Clippy passed; native interaction/capture 16/16 with exactly one recording clipboard write and other effects zero. [Qualification](../qualification/copy-search-paths-2026-10-28/README.md).

## Group selection in Find results (2026-10-28)

Adapted Double Commander multi-selection to Find: checkboxes/Cmd toggling, additive Shift ranges in visible order, separate Copy selected and Clear selection. Locations survive streaming/sorting; hiding deselects, new search clears, pane/file state remains separate. Regression distinguishes hard links and duplicate archive indexes. Full visual-qa suite 2528 passed / 3 ignored; Clippy passed; native checkbox/Shift-click/copy-selected capture 16/16 with exactly two recording clipboard writes and other effects zero. [Qualification](../qualification/search-result-selection-2026-10-28/README.md).

## Hide selected Find results (2026-10-28)

Extended Double Commander-style Remove from list to selected visible result groups. Batch set updates preserve unselected hard-link/archive locations, raw counts and files; hide clears affected selection/anchor/explanation/cursor and Show all restores without reselection. Controls wrap in narrow dialogs. Full visual-qa suite 2529 passed / 3 ignored; Clippy passed; native bulk-hide/restore sequence 16/16 with unchanged recording effects. [Qualification](../qualification/hide-selected-search-results-2026-10-28/README.md).

## Open selected Find results in background tabs (2026-10-28)

Adapted Double Commander multi-result Open In New Tab(s): selected visible targets open beside the active tab in displayed order, preserving both live panes and Find. Entire group preflight rejects capacity/guard/lexical-target failure without partial opening. Lazy target focus, per-tab hidden view and repeated archive containers retained. Both-pane/invalid/blocked/capacity/order regressions passed; full visual-qa suite 2531 passed / 3 ignored, Clippy passed, native group-open capture 16/16 with five tabs and unchanged effect counts. [Qualification](../qualification/selected-result-background-tabs-2026-10-28/README.md).

## Select all / invert visible Find results (2026-10-28)

Adapted Double Commander Mark commands to Find: Select all visible and Invert selection operate on received non-hidden locations, preserve separate hard-link/archive rows and leave streaming additions unselected. Range anchors reset; pane/cursor/text-editing behavior stays independent. Full visual-qa suite 2532 passed / 3 ignored; Clippy passed; native intermediate-state/selection/group-actions capture 16/16 with unchanged effect counts. [Qualification](../qualification/visible-result-selection-2026-10-28/README.md).

## Remember / restore Find result selection (2026-10-28)

Adapted Double Commander internal-buffer Save/Restore Selection to Find: one root-bound runtime location snapshot survives query refinements/hiding and restores only matching current visible results. Wrong roots and pending root edits with old results cannot rebind it; new snapshot replaces old, closing Find discards it. Full visual-qa suite 2533 passed / 3 ignored; Clippy passed; native remember/hide/restore-selection capture 16/16 with unchanged effect counts. [Qualification](../qualification/find-selection-memory-2026-10-28/README.md).

## Find clipboard output formats (2026-10-28)

Adapted Nimble Commander name/path and terminal filename workflows into Find Copy as: Full paths, Names and POSIX Shell arguments for visible/selected results. Shell uses real archive containers and space-joined individually quoted arguments; normal paths retain virtual members. Actual shell parser regression preserves apostrophes/newlines/substitution-looking text. Full visual-qa suite 2534 passed / 3 ignored; Clippy passed; native picker/payload capture 16/16 with exactly four recording clipboard writes and other effects zero. [Qualification](../qualification/find-copy-formats-2026-10-28/README.md).

## Find metadata CSV copying (2026-10-28)

Adapted Double Commander Copy all shown columns as Find Details (CSV): full virtual path, basename, file/folder kind, exact byte-size text and fractional UTC timestamp, with blank folder sizes/unknown dates. Existing CSV escaping reused; shared timestamp conversion hardened against out-of-range dates while pane precision stays unchanged. Full visual-qa suite 2536 passed / 3 ignored; Clippy passed; native picker/payload capture 16/16 with exactly five recording clipboard writes and other effects zero. [Qualification](../qualification/find-details-csv-2026-10-28/README.md).

## Find result context menu (2026-10-28)

Adapted Double Commander result context actions: reveal/open/background/group tabs, row name/path copying, row/group hiding and explanations. Right-click preserves selected groups or solely selects an outside row; row copies override output format without changing it. Uses existing guarded dispatch and query focus. Full visual-qa suite 2537 passed / 3 ignored; final Clippy/format/build passed; native selected/unselected right-click/copy sequence 16/16 with exactly seven recording clipboard writes and other effects zero. [Qualification](../qualification/find-result-context-menu-2026-10-28/README.md).

## Precise elapsed modification-age search (2026-10-28)

Adapted fd duration filters as age: with seconds/minutes/hours/days/weeks, strict/inclusive/equality boundaries, checked whole-duration parsing and saved-query semantics. Existing date: whole-day/month behavior preserved; unknown times fail and future times clamp to zero. Nanosecond-boundary/all-mode serialization and real native/index/TAR matrix passed. Full visual-qa suite 2539 passed / 3 ignored; Clippy passed. [Qualification](../qualification/precise-age-query-2026-10-28/README.md).

## Exact size-query numeric bounds (2026-10-28)

Closed two reproduced defects in the fd-inspired size-filter transfer: strict >MAX saturation and decimal f64 precision loss. Integer decimal/unit arithmetic now rounds by the existing nearest-byte policy with checked limits, preserving valid units/saved integer bounds. Max/2^53/long-fraction/rounding regressions and real native/index/ZIP threshold agreement qualified. Full visual-qa suite 2542 passed / 3 ignored; Clippy passed. [Qualification](../qualification/exact-size-numeric-bounds-2026-10-28/README.md).

## Ordinary-file search (2026-10-28)

Adapted fd regular-file classification as type:regular/type:regular-file across lexical native metadata, cached index classification, ZIP and TAR. Preserves type:file compatibility; old index unknown kinds require rebuild and count as skipped. All-mode native/index/ZIP/TAR matrix excludes links/FIFO/socket and retains native hard links. Full visual-qa suite 2544 passed / 3 ignored; Clippy passed. [Qualification](../qualification/regular-file-query-2026-10-28/README.md).

## File-only search exclusions (2026-10-28)

Adapted Double Commander Find Exclude files as excludefile: whole-name masks. Non-directory matches are filtered while matching directories/children and filtered archive-container members remain searchable. Case policy, repeated masks, saved-query semantics and explanations share the compiled provider path; existing exclude: pruning is preserved. Native/JSON-index/ZIP/TAR all-mode regressions passed; full visual-qa suite 2546 passed / 3 ignored, Clippy passed. [Qualification](../qualification/file-only-exclusions-2026-10-28/README.md).

## Observed Unix permission search (2026-10-28)

Adapted Double Commander Unix permission attributes as symbolic perm: requirements covering twelve owner/group/others/special bits. Required/forbidden conjunctions compile once and use lexical native, cached-index or ZIP/TAR mode metadata; missing observations count as skipped. Does not infer effective access/ACLs. All 4096 mode combinations, all-mode provider matrix, own symlink bits, special modes, cached post-chmod behavior and old index accounting qualified. Metadata overhead allowance increased to 640 after measuring 603 bytes; serialized-budget regression passed. Final full visual-qa suite 2549 passed / 3 ignored; Clippy passed. [Qualification](../qualification/permission-query-2026-10-28/README.md).

## Permission alternatives, group negation and octal masks (2026-10-28)

Extended the Double Commander Unix attribute transfer with OR groups, leading whole-group negation and signed octal masks. Terms remain AND within groups and repeated perm: conditions; unknown modes never match under negation. Empty/contradictory/malformed/over-budget patterns are rejected. Exhaustive 4096-mode truth tables/equivalence, expression/JSON and native/index/ZIP/TAR all-mode matrix passed. Full visual-qa suite 2551 passed / 3 ignored; Clippy passed. [Qualification](../qualification/permission-query-groups-2026-10-28/README.md).

## Directory-only search exclusions (2026-10-28)

Adapted Double Commander Exclude subdirectories as excludedir: whole-name masks. Native traversal prunes lexical directories; indexed/ZIP/TAR paths filter real directory ancestors, including implicit archive folders. Named files/symlinks and archive containers remain eligible; root and virtual-container scope are preserved. Case/quoted/repeated masks, saved queries, native/index/archive matrices and symlink regressions passed. Full visual-qa suite 2555 passed / 3 ignored; Clippy passed. [Qualification](../qualification/directory-only-exclusions-2026-10-28/README.md).

## Root-relative directory exclusion masks (2026-10-28)

Extended the Double Commander directory exclusion transfer with rooted relative paths, component-local wildcards, whole-component globstar, ./ anchoring and optional trailing slash. Basename masks preserve their any-depth behavior; provider scopes distinguish native paths from archive members and never insert virtual container prefixes. Invalid absolute/traversal/empty/embedded-globstar paths are rejected. Native/JSON-index/ZIP/TAR all-mode, Unicode, case, zero/many-level globstar and saved-query regressions passed; full visual-qa suite 2557 passed / 3 ignored, Clippy passed. [Qualification](../qualification/relative-directory-exclusions-2026-10-28/README.md).

## Absolute Unix directory exclusion masks (2026-10-28)

Extended the Double Commander directory exclusion transfer with leading-/ lexical Unix masks. Native and indexed directory ancestry use filesystem-root anchoring; archive member paths remain relative and virtual container components never participate. Matching files and archive containers remain eligible, while archives stored below excluded native branches are skipped. Root exemption, wildcard/component/case/invalid syntax, JSON and native/index/ZIP/TAR matrices passed. Full visual-qa suite 2559 passed / 3 ignored; Clippy passed. [Qualification](../qualification/absolute-directory-exclusions-2026-10-28/README.md).

## Literal whole-word content search (2026-10-28)

Adapted ripgrep fixed-string word boundaries as contentword:/notcontentword: literal predicates, independent of text mode and respecting Unicode/case/saved-query semantics. Native/index/ZIP/TAR share the compiled matcher. Unknown EOF and split UTF-8 tails cannot manufacture boundaries; safe prefix hits remain eligible, unresolved prefixes count as skipped and negative conditions require complete text. Real 16-MiB cap/UTF-8/EOF, legacy-cache, Unicode/punctuation/ranges and all-mode provider regressions passed. Full visual-qa suite 2563 passed / 3 ignored; Clippy passed. [Qualification](../qualification/whole-word-content-2026-10-28/README.md).

## Literal whole-line content search (2026-10-28)

Adapted ripgrep fixed-string line matching as contentline:/notcontentline: conditions, independent of text mode and respecting case/spaces. Empty values find actual blank lines; empty files and phantom EOF lines are distinguished. LF/CR/CRLF, shared bounded-context/EOF/completeness gates, literal escaping, saved queries and native/index/ZIP/TAR matrices qualified, including real 16-MiB and unknown-cache fixtures. Full visual-qa suite 2567 passed / 3 ignored; Clippy passed. [Qualification](../qualification/whole-line-content-2026-10-28/README.md).

## Smart-case search (2026-10-28)

Adapted ripgrep smart case as opt-in case:smart with one global policy across case-controlled conditions. Unicode uppercase literals enable sensitivity; parsed regex analysis ignores class/capture/comment syntax and preserves no-literal class behavior. Structural values do not participate; existing explicit modes and saved representations remain compatible. Promoted already-locked regex-syntax 0.8.11 to a direct dependency without package upgrades. All-mode native/index/ZIP/TAR, regex/escaped/range/inline-flag, structural/default/conflict and serialization regressions passed. Final full visual-qa suite 2571 passed / 3 ignored; Clippy passed. [Qualification](../qualification/smart-case-search-2026-10-28/README.md).

## Explicit dot-hidden search policy (2026-10-28)

Adapted ripgrep hidden traversal control as hidden:include/exclude/only, preserving Commander's inclusive default. Exclude prunes native hidden branches and filters cached/member paths; Only keeps visible traversal to discover hidden descendants and inherits hidden archive/container ancestry. Explicit-root exemption, aliases/conflicts, saved queries, smart-case independence, raw-name detection and native/index/ZIP/TAR matrices qualified. Dot policy is independent of pane visibility and OS hidden attributes. Full visual-qa suite 2575 passed / 3 ignored; Clippy passed. [Qualification](../qualification/hidden-search-policy-2026-10-28/README.md).

## BOM-aware Unicode content search (2026-10-28)

Adapted ripgrep BOM/transcoding search with shared native/index/ZIP/TAR decoding for UTF-8 BOM, UTF-16LE/BE and an owned UTF-32LE/BE extension. Signature-aware NUL sniffing preserves ordinary binary rejection, malformed units stay lossy and bounded surrogate tails preserve word/line safeguards. Index quota preflight now admits decoded text that fits while actual UTF-8 length still enforces the hard quota; reproduced BOM-overhead failure retained. Literal goldens, endian/payload-BOM/malformed tests, all-mode provider matrix, real 16-MiB surrogate cut and quota shrink/expansion qualified. Full visual-qa suite 2581 passed / 3 ignored; Clippy passed. [Qualification](../qualification/bom-content-search-2026-10-28/README.md).

## Alternative content patterns (2026-10-28)

Adapted ripgrep repeated patterns as anycontent: fields coalesced into one bounded OR group. Other predicates and repeated content: remain AND; members share mode/global case and first-declared successful score/ranges/explanation with a single content read. Group bounds, canonical/saved state, regex validation, smart-case analysis, delimiters, incomplete negative gates and native/index/ZIP/TAR/BOM-content matrices qualified. Full visual-qa suite 2585 passed / 3 ignored; Clippy passed. [Qualification](../qualification/alternative-content-search-2026-10-28/README.md).

## Find file-URL copying (2026-10-28)

Adapted Nemo URI clipboard representation into Find Copy as File URLs, reusing Commander's existing owned percent encoder. Visible/selected order, hidden omissions, real archive containers, repeated targets and raw Unix bytes are preserved; output is text links rather than file-transfer clipboard data. Full visual-qa suite 2587 passed / 3 ignored; Clippy/build passed. Fresh native picker/copy capture 16/16 with exactly eight recording clipboard writes, query focus/selection/tabs retained and other effects zero. [Qualification](../qualification/find-file-urls-2026-10-28/README.md).

## Find names without extension (2026-10-28)

Adapted Yazi copy name_without_ext into Find Copy as Names (no extension), reusing Commander's owned lexical formatter. Last suffix removal preserves dotfiles, member names, order, duplicates, hidden omission and control escaping for visible/selected results. Full visual-qa suite 2588 passed / 3 ignored; Clippy/build passed. Fresh native picker/selected/all copying capture 16/16 with ten recording clipboard writes, unchanged query focus/selection/tabs and other effects zero. [Qualification](../qualification/find-names-without-extension-2026-10-28/README.md).

## Find parent folders (2026-10-28)

Adapted Yazi copy dirpath into Find Copy as Parent folders for visible/selected scopes. Real archive-container folders, stable native-path deduplication, folder counts, root omission and control-rendering collisions qualified. Full suite 2590 passed / 3 ignored; Clippy passed. [Qualification](../qualification/find-parent-folders-2026-10-28/README.md).

## Alternative filename patterns (2026-10-28)

Adapted Double Commander alternative filename searching as repeated anyname: terms, one bounded OR group independent of anycontent: and combined with other predicates through AND. Existing Exact/Fuzzy/Regex and global case policy apply; first declared match supplies score/ranges/explanation. Canonical/saved state, limits, mode/literal/smart-case conditions and real native/index/ZIP/TAR matrices qualified. Full visual-qa suite 2594 passed / 3 ignored; all-target Clippy passed. [Qualification](../qualification/alternative-name-search-2026-10-28/README.md).

## Whole literal filename conditions (2026-10-28)

Adapted fd --exact as nameexact:/notnameexact: predicates independent of text mode. Entire-basename literal matching uses global case policy, literal smart-case evidence, complete character highlights and saved query support. Punctuation/Unicode/line-control/empty-value regressions plus native/index/ZIP/TAR matrix passed. Full visual-qa suite 2597 passed / 3 ignored; all-target Clippy passed. [Qualification](../qualification/exact-name-search-2026-10-28/README.md).

## Special file-type search (2026-10-28)

Adapted fd FIFO/socket/block/character filters as lexical type:fifo/pipe, type:socket, type:block-device/block and type:char-device/char. Native Unix metadata, optional validated cached type facts and ZIP/TAR header kinds feed the shared matcher; old/invalid facts are skipped rather than guessed. ZIP special payloads now skip text inspection, preserving native/TAR regular-content safeguards. Real FIFO/socket, symlink, all-mode native/index/ZIP/TAR and unknown-cache matrices passed. Full suite 2600 passed / 3 ignored; Clippy passed. No privileged device node created. [Qualification](../qualification/special-file-type-search-2026-10-28/README.md).

## Explicit type alternatives (2026-10-28)

Adapted fd multiple-type OR as types:regular,symlink and types:fifo,socket lists, supporting all existing type values and aliases while preserving ordinary/repeated-list conjunctions. Shared nonrecursive type parsing, 1–64 bounds, saved/chip canonical forms and three-state observed-metadata OR avoid discarding known matches because another branch is unknown. Expanded all-mode native/index/ZIP/TAR and legacy partial-fact matrices passed; full suite 2602 passed / 3 ignored, Clippy passed. [Qualification](../qualification/type-alternatives-search-2026-10-28/README.md).

## Empty files and directories (2026-10-28)

Adapted fd type empty as type:empty and an alternative in types: lists. Native lexical size/first-entry observations, optional cached facts and bounded complete archive topology distinguish empty ordinary files/folders from links/specials and folders with hidden/excluded children. Unknown old/read-error/capped/unsafe evidence remains skipped; known OR branches and partial safe zero-byte results survive. All-mode native/index/ZIP/TAR, stale-index, normalization and real archive-cap regressions passed. Full suite 2607 passed / 3 ignored; Clippy passed. [Qualification](../qualification/empty-file-directory-search-2026-10-28/README.md).

## Sequential rename replacement pairs (2026-10-28)

Adapted Double Commander multiple Find/Replace pairs into explicit sequential rows in batch rename. Up to sixteen extra pairs share matching options and target, persist in presets, and compose with existing transforms. Bounded capture interpolation and a distinct expansion-limit status block the entire batch before writes. Engine, cached preview, preset and real Undo/Redo/collision/partial-application regressions passed: full suite 2613 passed / 3 ignored, Clippy and build passed. Native verification recovered on 2026-10-30: the delayed run and a fresh corrected-glyph capture each passed 16/16 with native effects zero. Full tests/Clippy/build were rechecked; both sequential rows and Final.txt preview were visually confirmed. [Qualification](../qualification/rename-replacement-chain-2026-10-28/README.md).

## Reorder additional rename pairs (2026-10-30)

Independent usability follow-up to the adapted multiple-pair rename workflow: Up/Down swaps complete additional pairs, refreshes the preview and retains submitted/saved order. Boundary actions disable and controls fit the existing dialog. Full suite 2614 passed / 3 ignored; Clippy/build passed. Native Down→Doc.txt and Up→Final.txt checks reached phase 68, capture 16/16 with effects zero. [Qualification](../qualification/rename-chain-order-2026-10-30/README.md).

## Reorder files before batch numbering (2026-10-30)

Adapted Double Commander’s reorderable Multi-Rename file list with Up/Down preview-row controls. Complete target swaps retain scope/membership/kinds and invalidate cached ordinal plans; same-frame submission is suppressed. Order is runtime batch context, not preset data. Cached-numbering and real payload/Undo/Redo regressions passed. Full suite 2616 passed / 3 ignored; Clippy/build passed. Native two-target Down/Up renumbering reached phase 82, capture 16/16 with effects zero. [Qualification](../qualification/rename-target-order-2026-10-30/README.md).

## Exclude files from a rename batch (2026-10-30)

Adapted Double Commander’s removal from the Multi-Rename list as Exclude preview-row actions. Files remain untouched, counters compact, excluded sibling names still block collisions, and removing the final target closes the empty batch. Deferred exclusion invalidates validation and suppresses stale same-frame submit. Counter/collision/empty-set and real payload/Undo regressions passed. Full suite 2619 passed / 3 ignored; Clippy/build passed. Native exclusion left notes.md→01.md at phase 85, capture 16/16 with effects zero. [Qualification](../qualification/rename-target-exclusion-2026-10-30/README.md).

## Copy batch rename plans (2026-10-30)

Independent clipboard adaptation of Double Commander's old/new-name list workflow: copy original names, validated proposed names, or CSV old/new/status/error mapping. Current row order and duplicate text are retained; plain lists escape line controls and CSV quotes raw fields. Invalid plans remain available for diagnostic export, while proposed-name export is disabled. Native clicks after exclusion/renumbering verified exact payloads at phase 97: 16/16, three recording clipboard writes and other effects zero. No list import/editor roundtrip. [Qualification](../qualification/rename-plan-copy-2026-10-30/README.md).

## Remove character ranges in batch rename (2026-10-30)

Independent adaptation of [Thunar Remove Characters](https://docs.xfce.org/xfce/thunar/bulk-renamer/start), cohort #24. Skip/count/from-end removes whole grapheme clusters from the transformed stem while preserving file extensions; short names clamp. Presets retain valid bounds, legacy rules default off, Reset clears it, and cached plans reflect edge changes. Six new regressions cover Unicode/ranges/order/validation/presets/cache and real payload/Undo/Redo/collision refusal. Full suite 2628 passed / 3 ignored; Clippy/build passed. Native start/end/disable sequence plus retained ordering/exclusion/export checks passed 16/16, with three recording clipboard calls and other effects zero. [Qualification](../qualification/rename-remove-range-2026-10-30/README.md).

## Insert or overwrite text during batch rename (2026-10-30)

Independent adaptation of [Thunar Insert or Overwrite characters](https://docs.xfce.org/xfce/thunar/bulk-renamer/start), cohort #24. Literal insertion uses a clamped grapheme anchor from either edge of the stem; overwrite replaces following clusters equal to the inserted text length. File extensions remain intact. Presets/cache/reset and existing validity/collision checks are integrated. Six new regressions cover Unicode/anchors/composition/invalid names/presets/cache and real payload/Undo/Redo/sibling collision refusal. Full suite 2634 passed / 3 ignored; Clippy/build passed. Native text-entry/overwrite/from-end/disable sequence plus retained checks passed 16/16, three recording clipboard calls and other effects zero. [Qualification](../qualification/rename-insert-text-2026-10-30/README.md).

## Choose positional character-edit scope (2026-10-30)

Independent adaptation of [Thunar name/suffix/both scope](https://docs.xfce.org/xfce/thunar/bulk-renamer/start), cohort #24. Range removal and insertion/overwrite can target stem (legacy default), raw extension or whole filename independently of Find/Replace. Extension edits skip directories and create/remove separators correctly; whole-name edits may change the suffix boundary. Case applies after character edits. Presets/reset/cache/legacy serde are integrated. Six regressions cover scopes/Unicode/validation/presets/cache and real payload/Undo/Redo/collision refusal. Full suite 2640 passed / 3 ignored; Clippy/build passed. Native scope switching under overwrite plus retained checks passed 16/16, with three recording clipboard calls and other effects zero. [Qualification](../qualification/rename-character-target-2026-10-30/README.md).

## Letter counters in batch rename (2026-10-30)

Independent adaptation of [Thunar alphabetic numbering option](https://raw.githubusercontent.com/xfce-mirror/thunar/master/plugins/thunar-sbr/thunar-sbr-enum-types.c), cohort #24. Lower/uppercase bijective Latin counters use 1=a, 26=z, 27=aa; zero stays 0 and negatives receive a sign. Existing signed step, target order and counter positions compose with the format; decimal-only padding is retained across switches. Presets/reset/cache/legacy serde are integrated. Six regressions cover boundaries/extrema/composition/presets/cache and real payload/Undo/Redo/collision refusal. Full suite 2646 passed / 3 ignored; Clippy/build passed. Native lowercase/uppercase/Decimal switching plus retained checks passed 16/16, with three recording clipboard calls and other effects zero. [Qualification](../qualification/rename-letter-counters-2026-10-30/README.md).

## Keep a positional name range (2026-10-30)

Independent adaptation of [Double Commander name/extension range extraction](https://doublecmd.github.io/doc/en/multirename.html), cohort #3. Range now offers Remove (legacy default) or Keep, using existing grapheme-safe skip/count/from-end and component scope. Extraction composes before insertion/case/decoration and retains final validity/collision guards. Presets/reset/cache/legacy serde are integrated. This is a single range, not the complete placeholder grammar. Six regressions cover edges/Unicode/scopes/composition/validation/presets/cache and real payload/Undo/Redo/collision refusal. Full suite 2652 passed / 3 ignored; Clippy/build passed. Native Remove/Keep/edge switching plus retained checks passed 16/16, with three recording clipboard calls and other effects zero. [Qualification](../qualification/rename-keep-range-2026-10-30/README.md).

## Full-path folder-tab labels (2026-10-30)

Independent per-tab adaptation of [Krusader fullpath tab names](https://docs.kde.org/trunk_kf6/en/krusader/krusader/konfig-panel.html), cohort #4. Show full folder path overrides the short/custom label while preserving the title, using bounded grapheme-safe path truncation and complete hover/accessibility text. Metadata survives duplication/copy/move/navigation/close-reopen/session restore; launch-overridden active paths do not inherit it. Four new regressions plus extended lifecycle coverage passed: full suite 2656 passed / 3 ignored; Clippy/build passed. Dedicated local-fixture native on/off/on interaction on an inactive tab passed 14/14, effects zero. The broader FolderTabs run blocked in existing home-directory watcher scanning; stack evidence is retained rather than counting it as passed. [Qualification](../qualification/full-path-tab-label-2026-10-30/README.md).

## Complete watcher setup outside the UI thread (2026-10-30)

Responsiveness follow-up to folder-tab transfer qualification: mount profiling, canonicalization, initial polling scans and native subscription creation now run in background workers. The UI owns only a stop-sender lease; backend teardown also stays on its owner thread. Eight admission slots bound unfinished installs, stale epochs cannot activate a new directory, and completion reconciles changes before subscription. Native promotion failure retains polling; worker panic retires dead subscriptions. Deterministic blocked-worker/admission/race/failure tests and actual filesystem observation passed. Full suite 2661 passed / 3 ignored; Clippy/build passed. The original FolderTabs scenario, including previously blocked Home/Root navigation and full-path labels, now completed 14/14 with effects zero. [Qualification](../qualification/async-watcher-install-2026-10-30/README.md).

## SHA-384 and SHA-512 verification (2026-10-30)

Independent adaptation of [Double Commander checksum algorithm choices](https://github.com/doublecmd/doublecmd/blob/master/src/fchecksumcalc.pas), cohort #3. SHA-384/512 join the fresh single-read stream with existing hashes using the existing sha2 dependency. Explicit comparison validates 96/128 hex characters; selected digest and GNU-format SHA lists can be copied. Identity/cancellation/no-follow guards remain unchanged, and incomplete-report matrices now cover all algorithms. Known-vector and system shasum tests include multi-buffer and escaped-name files. Full suite 2664 passed / 3 ignored; Clippy/build passed. Native selection/comparison/digest/list checks passed 14/14 with four recording clipboard writes and other effects zero. No hashing-speed or full upstream parity claim. [Qualification](../qualification/sha384-sha512-checksums-2026-10-30/README.md).

## Hex viewer percentage Go to (2026-10-30)

Independent Rust adaptation of [Midnight Commander percentage navigation](https://github.com/MidnightCommander/mc/blob/master/src/viewer/dialogs.c), cohort #40. Offset accepts whole 0% through 100%, rounds down against the loaded file size with overflow-free arithmetic, and retains stamp validation and bounded reads. Invalid percentages are rejected; 100% opens EOF. Two new regressions cover all percentages through maximum-u64 sizes, real unaligned reads and changed-file rejection. Full suite 2666 passed / 3 ignored; Clippy/build passed. Native Next/Previous/absolute/decimal plus 50% → 100% → 0% passed 14/14 with effects zero. [Qualification](../qualification/hex-percentage-navigation-2026-10-30/README.md).

## Hex viewer session bookmarks (2026-10-30)

Independent Rust adaptation of [Midnight Commander viewer marks](https://github.com/MidnightCommander/mc/blob/master/src/viewer/actions_cmd.c), cohort #40. Ten numbered per-viewer slots store exact page offsets, with Set here, Jump and Clear controls. Reload clears all slots; jumps retain file-stamp validation and bounded asynchronous reads. Regression coverage includes slot isolation/overwrite, EOF, actual byte restoration, changed files and reload. Full suite 2668 passed / 3 ignored; Clippy/build passed. Native Set → Next → Jump → Clear interaction passed 14/14, effects zero. [Qualification](../qualification/hex-viewer-bookmarks-2026-10-30/README.md).

## UTF-8 text search in hex viewer (2026-10-30)

Independent adaptation of [Double Commander viewer text/hex search](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Explicit UTF-8 mode searches exact encoded bytes, including meaningful whitespace and case, in arbitrary binary files. Existing bounded forward/backward search, progress/cancellation and stamp guards are shared. Two regressions cover Unicode/byte limits, actual page/chunk crossings, reverse search and case sensitivity. Full suite 2670 passed / 3 ignored; Clippy/build passed. Native original hex searches followed by UTF-8 forward/forward/backward passed 14/14, effects zero. This does not claim encoding/regex/case-insensitive viewer parity. [Qualification](../qualification/hex-utf8-search-2026-10-30/README.md).

## UTF-16 LE/BE search in hex viewer (2026-10-30)

Independent Rust adaptation of [Double Commander encoding-aware viewer search](https://github.com/doublecmd/doublecmd/blob/master/components/viewer/viewercontrol.pas), cohort #3. Explicit UTF-16 LE/BE search encodes exact text, including surrogate pairs, without adding a BOM. Both modes retain bounded streaming reads, stamp checks, cancellation and forward/backward search; odd embedded byte offsets work. Two regressions cover known endian vectors, NUL/BOM/encoded limits and actual page/chunk crossings. Full suite 2672 passed / 3 ignored; Clippy/build passed. Native retained hex/UTF-8 sequence plus LE/BE forward/forward/backward passed 14/14 with effects zero. [Qualification](../qualification/hex-utf16-search-2026-10-30/README.md).

## DOCX and ODT text previews (2026-10-30)

Independent Rust adaptation of [Double Commander Office XML text viewing](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Bounded DOCX/ODT body extraction preserves paragraphs, Unicode, tabs/breaks and table text through the existing cancellable identity-checked text pipeline. Existing find/copy controls work on extracted text. Package/XML/output/node/depth bounds and DTD-disabled parsing apply; no resources are fetched or extracted to disk. Four regressions cover both formats, strict/prefix-independent XML, malformed/oversized input, cancellation and file changes. Full suite 2676 passed / 3 ignored; Clippy/build passed. Fresh native DOCX and ODT captures each passed 14/14, with exact copied text and one recording clipboard call. Office layout/images/notes and spreadsheet formats remain open. [Qualification](../qualification/office-text-preview-2026-10-30/README.md).

## XLSX and ODS text previews (2026-10-30)

Independent Rust adaptation of [Double Commander Office XML spreadsheet viewing](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Named sheets and literal/cached values appear in workbook order using existing find/copy controls. XLSX resolves internal worksheet/shared-string relationships and sparse cell references; ODS expands bounded repetitions while omitting compact empty tails. No formula execution or external-resource access. Aggregate XML/output/node/depth/coordinate limits apply; DOCX/ODT retain shared package guards. Eight regressions passed; full suite 2684 passed / 3 ignored; Clippy/build passed. Fresh native XLSX and ODS captures each passed 14/14 with exact copied text. Formatting/charts/recalculation remain open. [Qualification](../qualification/spreadsheet-text-preview-2026-10-30/README.md).

## PPTX and ODP slide text previews (2026-10-30)

Independent Rust adaptation of [ranger ODP text previews](https://github.com/ranger/ranger/blob/master/ranger/data/scope.sh), cohort #30, additionally extended here to PPTX. Ordered slide text, names, paragraphs and tables use existing find/copy controls. PPTX follows internal slide-list relationships, including strict namespaces; ODP omits notes/comments. Common bounded XML parsing and internal part paths are shared with other Office previews. No rendering/playback or external resources. ODP/ODS now classify as Documents. Five new regressions plus extended kind coverage passed; full suite 2689 passed / 3 ignored; Clippy/build passed. Fresh native PPTX and ODP captures each passed 14/14 with exact copied text. [Qualification](../qualification/presentation-text-preview-2026-10-30/README.md).

## EPUB spine text previews (2026-10-30)

Independent Rust adaptation of [ranger EPUB text conversion previews](https://github.com/ranger/ranger/blob/master/ranger/data/scope.sh), cohort #30. Container/OPF metadata resolves UTF-8 HTML/XHTML chapters in spine order, with numbered chapter headings and existing find/copy controls. HTML tokenization decodes entities and excludes executable/embedded content; no converters or resources are opened. URI-encoded package paths, encryption metadata and aggregate budgets are checked. EPUB now classifies as Documents. Five new regressions plus expanded type coverage passed; full suite 2694 passed / 3 ignored; Clippy/build passed. Fresh native EPUB capture passed 14/14 with exact copied text. Pagination/media/CSS/DRM remain open. [Qualification](../qualification/epub-text-preview-2026-10-30/README.md).

## FB2 book text previews (2026-10-30)

Independent Rust adaptation of [ranger FictionBook text previews](https://github.com/ranger/ranger/blob/master/ranger/data/scope.sh), cohort #30. Raw FB2 body/title/verse/table extraction uses existing find/copy controls and preserves named notes bodies. Strict UTF-8/16 and declared Windows-1251/1252, KOI8-R/U, ISO-8859-5 decoding rejects bad bytes and conflicting BOMs. XML/node/depth/output limits and existing preview identity/cancellation guards apply; binaries/resources stay inert. FB2 classifies as Documents. Five regressions plus type coverage passed; full suite 2699 passed / 3 ignored; Clippy/build passed. Fresh native Windows-1251 FB2 capture passed 14/14 with exact copied text. Compressed FB2/layout/images remain open. [Qualification](../qualification/fb2-text-preview-2026-10-30/README.md).

## Whole-document PDF text search (2026-10-30)

Independent Rust extension of [ranger multi-page PDF text previews](https://github.com/ranger/ranger/blob/master/ranger/data/scope.sh), cohort #30. Native PDFKit scans all pages through one worker-owned document, retaining only matching page/count records. Literal Unicode/case search has progress/cancel; selecting a result opens the correct text page with the same query highlighted. No all-document text galley, OCR or partial-success claim on page errors. Query/page/text/count limits apply, close cancels and rejects late results, existing PDF admission/size/mtime/timeout guards remain. Five new regressions passed; full suite 2704 passed / 3 ignored; Clippy/build passed. Fresh native PdfSearch and original PdfPreview captures each passed 14/14 with effects zero. [Qualification](../qualification/pdf-document-search-2026-10-30/README.md).

## PDF document regex and whole-word search (2026-10-30)

Independent adaptation of [Double Commander viewer regex search](https://doublecmd.github.io/doc/en/viewer.html), cohort #3, and [ripgrep whole-word search](https://github.com/BurntSushi/ripgrep/blob/master/GUIDE.md), cohort #76. Global PDF Regex/Whole words options snapshot into results and transfer to page find. A shared bounded compiler keeps page/global semantics aligned, including existing full Unicode boundaries and trailing free-spacing comment handling. Invalid patterns fail before text callbacks; blank pages are omitted. Two new regressions and all preview regressions passed; full suite 2706 passed / 3 ignored; Clippy/build passed. Fresh native PdfRegex and literal PdfSearch each passed 14/14, effects zero. No claim of ripgrep half-boundary or full regex dialect parity. [Qualification](../qualification/pdf-regex-word-search-2026-10-30/README.md).

## PDF document Smart case (2026-10-30)

Independent adaptation of [lf smartcase](https://github.com/gokcehan/lf/blob/master/doc.md#smartcase-bool-default-true), cohort #32. Whole-document PDF search gains an optional Smart case flag, shared Unicode policy with page find, and exact flag transfer on result navigation. Manual Match case wins; existing preview regex syntax/inline-flag behavior is retained, without claiming ripgrep regex-literal-analysis parity. Two new regressions cover Unicode, manual/inline overrides, default-off snapshots and editing the transferred page query. Native QA exposed a 7.5 px pane overflow from ScrollArea's default minimum; page text now fits remaining height. Also removed a production constructor dependency on a QA-only helper. Full suite 2708 passed / 3 ignored; default locked check, Clippy/build passed. Fresh PdfRegex and PdfSearch each 14/14 with zero native effects; frames inspected. [Qualification](../qualification/pdf-smart-case-search-2026-10-30/README.md).

## RTF text preview (2026-10-30)

Independent Rust adaptation of [ranger RTF text previews](https://github.com/ranger/ranger/blob/master/ranger/data/scope.sh), cohort #30. A bounded parser replaces raw control-word display with visible body/paragraph/table text, scoped Unicode fallback and surrogate handling, strict document/font codepages and Unicode alternatives. Metadata, field instructions, objects/pictures, unknown starred destinations and hidden/deleted text remain inert/omitted; visible field results retain text. Existing worker, identity guards, find and copy own publication. Eight regressions cover decoding, fallback/group semantics, multibyte font priority, inert binary/destinations, malformed/truncated files, bounds/cancellation and the actual filesystem provider. Full suite 2716 passed / 3 ignored; default locked check, Clippy/build passed. Native RtfPreview passed 14/14 with one exact recording clipboard write, frame inspected. No full RTF/layout/RTFD compatibility claim. [Qualification](../qualification/rtf-text-preview-2026-10-30/README.md).

## Cyclic hex viewer search (2026-10-30)

Independent adaptation of [lf wrapscan](https://github.com/gokcehan/lf/blob/master/doc.md#wrapscan-bool-default-true), cohort #32: applies the file-list interaction to Commander's binary content scanner. Optional Wrap search, default off, crosses either edge in all four input modes and reports Wrapped. Disjoint candidate ranges avoid duplicate candidate scanning while retaining chunk/split context; EOF/BOF bytes never concatenate into a match. Both passes retain one worker/descriptor, cancellation and final file/page identity guards. Five new regressions cover naive exhaustive ordering, boundaries/read bounds, cancellation, all modes and file replacement with/without matches. Full suite 2721 passed / 3 ignored; default check, Clippy/build passed. Extended native HexSearch includes both wrapped directions and passed 14/14, effects zero, frame inspected. No claim that lf contains this exact binary scanner. [Qualification](../qualification/hex-wrap-search-2026-10-30/README.md).

## Bzip2, XZ and Zstandard TAR (2026-10-30)

Independent integration of [ouch compressed TAR formats](https://github.com/ouch-org/ouch/blob/main/README.md), cohort #97. Browser and live/indexed content search now admit tar.bz2/tbz/tbz2, tar.xz/txz and tar.zst/tzst/tar.zstd. Streaming decoders share TAR budgets/path rules/tree/sorting and cancellation, with XZ memory and zstd window configuration. Selected extraction stages at most 128 MiB in a private auto-cleaned raw TAR before the existing system-tar path, so host zstd support is unnecessary. Five new regressions cover bounded/cancelled/concatenated/broken scans, staging, actual extraction/metadata/nonclobber and indexed filters; prior content/live tests now cover all five TAR kinds. Full suite 2726 passed / 3 ignored; default check, Clippy/build passed. Fresh native three-codec and original TAR captures each 14/14, effects zero, frames inspected. Static XZ linking and binary dylib inspection avoid new host compression-library dependencies. Other formats, full compatibility and other platforms remain outside this qualification. [Qualification](../qualification/compressed-tar-formats-2026-10-30/README.md).

## Archive member clipboard export (2026-10-30)

Independent adaptation of [Double Commander name/path/details clipboard commands](https://doublecmd.github.io/doc/en/cmds.html#catclipboard), cohort #3. Archive pane gains Names/Member paths/CSV and visible/selected scopes; selected includes hidden real members in current sort order, visible omits synthetic navigation folders. CSV preserves original member index, kinds, sizes, packed availability, UTC and truncation flag through shared formula-safe escaping. Output cap 4 MiB; stale/non-UTF-8 rows fail before clipboard publication. Four new regressions; full suite 2730 passed / 3 ignored, Clippy/default check/build passed. Native validation remains pending: initial QA guard confused original member index with sorted listing position, now fixed; CUA became unavailable after interruption before rerun. No successful capture or native clipboard claim. [Evidence and remaining gate](../qualification/archive-clipboard-export-2026-10-30/README.md).

## Archive Regex and Smart case filter (2026-10-30)

Independent adaptation of [Yazi regex/case filtering](https://github.com/sxyazi/yazi/blob/main/yazi-fs/src/filter.rs), cohort #27. Optional Regex, Match case and Smart case filter full internal paths, while the default retains literal/trimmed/Unicode-lowercase substring behavior. A bounded compiled pattern/error is cached by query/options and shared across real/navigation rows; invalid patterns hide all rows without losing selected members. Filtering away the focused row now clears its cursor, preventing stale Enter toggles; visible cursor and selection survive. Six new regressions; full suite 2736 passed / 3 ignored, Clippy/default check/build passed. Native window validation and the preceding ArchiveExport native gate remain pending because desktop CUA is unavailable; no native click/frame success claimed. [Evidence and remaining gates](../qualification/archive-regex-filter-2026-10-30/README.md).

## Read-only archive check (2026-10-30)

Independent adaptation of [Ark Test Integrity](https://github.com/KDE/ark/blob/master/part/part.cpp), cohort #99. Test archive / Stop check are wired to a read-only worker: ZIP CRC/overlap checks or TAR headers, bounded logical payloads and full compressed streams, including later frames. Limit/cancel/unsupported/corruption outcomes never produce completed Reports. Regular-file descriptor/path guards reject replacement; a per-pane Run owns cancellation and excludes extraction. TAR's missing payload checksum is explicit and tested with a deliberately changed payload that remains structurally valid. Seven regressions; full suite 2743 passed / 3 ignored, Clippy/default check/build passed. Native Test/Stop/layout validation remains pending while CUA is unavailable; previous export/filter native gates stay open. [Evidence and limitations](../qualification/archive-integrity-check-2026-10-30/README.md).

## Archive Back / Forward history (2026-10-30)

Independent adaptation of [Yazi back/forward directory history](https://yazi-rs.github.io/docs/configuration/keymap/#back), cohort #27. Archive buttons revisit internal folders and All members, restoring a still-visible cursor while retaining current filter/sort and archive-wide selection. New navigation clears Forward; saved history is bounded to 64 locations and 256 KiB of encoded paths and resets with the listing. Four regressions cover traversal, branching, hidden cursors and bounds. Full suite: 2747 passed / 3 ignored; Clippy/default check/build and formatting passed. Native button/layout validation remains pending because desktop CUA is unavailable. No keyboard shortcut, pixel-scroll restoration, persistent history or full Yazi VFS parity claim. [Qualification](../qualification/archive-navigation-history-2026-10-30/README.md).

## Archive visible selection operations (2026-10-30)

Independent adaptation of [Double Commander invert/unmark commands](https://doublecmd.github.io/doc/en/cmds.html#catmark), cohort #3. Archive Invert visible / Clear visible change only current real rows, preserving hidden/cross-folder selections. Synthetic navigation rows remain excluded from extraction selection; duplicate member paths retain independent identities. Existing Clear still resets everything. Two regressions exercise actual folder/filter caches, repeated inversion, duplicate paths, hidden members, empty navigation-only views and invalid regex. Full suite: 2749 passed / 3 ignored; Clippy/default check/build and formatting passed. Native click/layout verification remains pending while desktop CUA is unavailable. [Qualification](../qualification/archive-visible-selection-2026-10-30/README.md).

## Archive additive range selection (2026-10-30)

Independent adaptation of [Double Commander Shift-click and Shift-arrow selection](https://doublecmd.github.io/doc/en/help.html#sel_files), cohort #3. Archive Shift-click and Shift-Up/Down in the filter add real members in current visible order from a stable anchor. Hidden selections survive, synthetic navigation endpoints are excluded and Shift gestures do not open folders. Ordinary clicks/arrows set a new anchor; query hiding and navigation reset stale anchors. Reversing a range is additive, not shrinking. Two regressions cover order, hidden selection, synthetic/stale endpoints and actual reset paths. Final-source full suite: 2751 passed / 3 ignored; Clippy/default check/build and formatting passed. Native gestures/layout remain pending while CUA is unavailable. [Qualification](../qualification/archive-range-selection-2026-10-30/README.md).

## Archive selection memory (2026-10-30)

Independent adaptation of [Double Commander SaveSelection / RestoreSelection](https://doublecmd.github.io/doc/en/cmds.html#catmark), cohort #3. Archive Save selection retains one explicit real-member snapshot including hidden/cross-folder selection. Restore replaces current selection, keeps duplicate identities and clears the range anchor. Saving empty is distinct from no snapshot; repeat Save overwrites. The buffer resets with its listing and is not persisted. Two regressions cover real filter/folder/history transitions, duplicates, repeated/empty restoration and reset. Full suite: 2753 passed / 3 ignored; Clippy/default check/build and formatting passed. Native clicks/layout remain pending while CUA is unavailable. [Qualification](../qualification/archive-selection-memory-2026-10-30/README.md).

## Archive extension sort (2026-10-30)

Independent adaptation of [Double Commander SortByExt](https://doublecmd.github.io/doc/en/cmds.html), cohort #3. Archive Sort gains case-insensitive final-suffix ordering across ZIP/all TAR codecs, with shared ascending default, Reverse, folders-first and natural-name tie breaks. Dotted directories, dotfiles, empty suffixes and duplicate identities remain explicit. Existing row-ID selection and export owners are retained. Regression covers suffix boundaries, reverse/folders-first, Unicode, duplicates, direction and codec availability. Full suite: 2754 passed / 3 ignored; Clippy/default check/build and formatting passed. Native menu/order validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-extension-sort-2026-10-30/README.md).

## Archive same-extension selection (2026-10-30)

Independent adaptation of [Double Commander MarkCurrentExtension / UnmarkCurrentExtension](https://doublecmd.github.io/doc/en/cmds.html#catmark), cohort #3. Same extension selects/deselects visible regular files from the focused file's case-insensitive final suffix, sharing archive sorting's key. Hidden/unmatched selections survive; directories, links and synthetic rows are excluded. Duplicate paths retain separate IDs, empty suffixes match, and hidden/stale cursors produce no changes. Two regressions cover folder/query ownership, case/Unicode/compound/empty suffixes, duplicates, exclusions and cache rejection. Full suite: 2756 passed / 3 ignored; final Clippy/default check/build and formatting passed. Native menu/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-same-extension-selection-2026-10-30/README.md).

## Archive selection narrowing (2026-10-30)

Independent adaptation of [ranger narrow](https://github.com/ranger/ranger/blob/master/ranger/config/commands.py), cohort #30. Narrow to selection snapshots real selected IDs and switches to All members while retaining query/sort. Deselecting rows leaves them visible for review; repeat Narrow refreshes. Remove narrowing or an empty selection restores ordinary listing without altering selection. Synthetic folders are omitted, duplicate IDs retained, and archive-wide selected actions keep hidden-count semantics. Two regressions cover real folder/query caches, stable snapshots, refresh, sorting, duplicates, empty/invalid IDs, navigation exclusion and regex errors. Final-source full suite: 2758 passed / 3 ignored; Clippy/default check/build and formatting passed. Native workflow/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-narrow-selection-2026-10-30/README.md).

## Archive member type filter (2026-10-30)

Independent adaptation of [ranger filter_inode_type](https://github.com/ranger/ranger/blob/master/ranger/config/commands.py), cohort #30. Archive Types combines regular files, directories, symbolic links and other records; all default on, with explicit reset and active indicator. Predicates intersect query/folder/narrowing, preserve hidden/saved selections and reuse existing metadata without member reads. Navigation folders stay available independently of type flags and remain excluded from real selection. Two regressions cover cache changes, combined flags, folder trees, exports/hidden selection, narrowing, regex failure and reset. Final-source full suite: 2760 passed / 3 ignored; final Clippy/default check/build and formatting passed. Native clicks/layout remain pending while CUA is unavailable. [Qualification](../qualification/archive-type-filter-2026-10-30/README.md).

## Archive selected-size summary (2026-10-30)

Independent adaptation of [Ark InfoPanel selected-size totals](https://github.com/KDE/ark/blob/master/part/infopanel.cpp), cohort #99. Archive selection summary counts files/directories/links/other records and exact u128 regular-file unpacked bytes, plus ZIP packed bytes or explicit TAR unavailability. Hidden choices and duplicate records count; directories do not expand implicitly, nonregular bytes are excluded. A per-listing selection-key cache avoids metadata rescans on unchanged repaints. Two regressions cover overflow-sized sums, duplicates, type counts, stale IDs, codec availability, hidden rows and cache reset. Final-source full suite: 2762 passed / 3 ignored; Clippy/default check/build and formatting passed. Native summary/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-selection-size-2026-10-30/README.md).

## Archive excluding path filter (2026-10-30)

Independent adaptation of [ranger filter-stack NotFilter](https://github.com/ranger/ranger/blob/master/ranger/core/filter_stack.py), cohort #30. Exclude matches negates valid literal/regex archive path matches, including navigation rows, while retaining type/folder/narrowing restrictions and selected member identities. Invalid patterns stay errors/empty; blank patterns match all and therefore exclude all. The flag invalidates visibility without recompiling unchanged queries; hidden cursor and hidden-selection counts follow existing owners. Two regressions cover caching, duplicates, Smart case, errors/empty queries, navigation and combined filters. Full suite: 2764 passed / 3 ignored; Clippy/default check/build and formatting passed. Native checkbox/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-exclude-filter-2026-10-30/README.md).

## Archive unified filter reset (2026-10-30)

Independent adaptation of [ranger filter_stack clear](https://github.com/ranger/ranger/blob/master/ranger/config/commands.py), cohort #30. Archive Reset filters clears query/regex/case/exclusion/types/narrowing while retaining folder, sorting, selection, saved selection and history. Explicit cache invalidation removes stale empty/error/narrowed views; the UI requests query focus. Two regressions cover simultaneous reset, preservation/idempotence, default/whitespace state and query-independent cache rebuild. Full suite: 2766 passed / 3 ignored; Clippy/default check/build and formatting passed. Native button/focus/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-reset-filters-2026-10-30/README.md).

## Archive member details (2026-10-30)

Independent adaptation of [Ark InfoPanel per-entry metadata](https://github.com/KDE/ark/blob/master/part/infopanel.cpp), cohort #99. A collapsed Member details panel exposes current real member kind, exact declared/ZIP packed bytes, UTC time, octal Unix permissions and executable flag from existing metadata. Known zero/false remains distinct from absent values; directory sizes are not implicit subtree totals. It shares the existing ID/path cursor/fallback owner and adds no payload reads, mutation or CSV schema change. Two regressions cover zero/absence, UTC, codec availability, special mode bits and record kinds. Full suite: 2768 passed / 3 ignored; Clippy/default check/build and formatting passed. Native expand/cursor/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-member-details-2026-10-30/README.md).

## Archive typed folder navigation (2026-10-30)

Independent adaptation of [Yazi interactive cd](https://yazi-rs.github.io/docs/configuration/keymap/#cd), cohort #27. Collapsed Go to folder accepts exact root-relative archive paths through Go/Enter. A bounded pure resolver handles virtual root/dot/slash normalization and binary-searches indexed directories, rejects parents/NUL/missing/file paths, preserves literal spaces/Unicode and never consults the host filesystem. Success uses existing deferred history-aware navigation without changing selection or filters; errors stay local. Two resolver regressions plus existing shared-history coverage apply. Full suite: 2770 passed / 3 ignored; Clippy/default check/build and formatting passed. Native Go/Enter/error/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-goto-folder-2026-10-30/README.md).

## Archive folder completion (2026-10-30)

Independent adaptation of [Yazi cd completion](https://yazi-rs.github.io/docs/configuration/keymap/#cmp), cohort #27. Go to folder Suggestions offers up to 12 exact indexed prefix matches plus overflow guidance, with cached draft results and direct history-aware navigation. Shared root-relative validation preserves spaces/Unicode and rejects parent/NUL/over-limit inputs; only resolvable UTF-8 directory paths are offered. No host access or fuzzy/full URL parity is claimed. Two regressions cover normalization, descendants, case, resolvability, deterministic caps/overflow and input/result limits. Full suite: 2772 passed / 3 ignored; Clippy/default check/build and formatting passed. Native menu/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/archive-folder-completion-2026-10-30/README.md).

## Hex Windows codepage search (2026-10-30)

Independent extension of [Double Commander viewer encoding selection](https://doublecmd.github.io/doc/en/viewer.html), cohort #3, to hex byte-search input. Explicit Windows-1251/1252 encode exact query bytes, retain spaces/case/NUL and reject unrepresentable/empty/over-limit input before work queues. Both modes reuse bounded directional/cyclic scanner, progress/cancel and file/page guards. Parser and no-worker-on-error regressions added; deterministic cyclic worker coverage expanded to both codepages across a chunk boundary. Final-source full suite: 2774 passed / 3 ignored; Clippy/default check/build and formatting passed. Native mode/layout/search validation remains pending while CUA is unavailable; historical HexSearch frames do not qualify new modes. [Qualification](../qualification/hex-windows-codepages-2026-10-30/README.md).

## Hex plain-byte clipboard export (2026-10-30)

Independent extension of [Double Commander viewer copying](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Copy bytes exports the loaded 4 KiB page as HEX/decimal values without offsets/ASCII, or strict UTF-8 with exact whitespace/controls. Invalid UTF-8 disables text action; no replacements, whole-file reads or freshness claims. Existing Copy page/text selection remain. Regressions cover all bytes/full-page bounds, round trips, nonzero/deleted-source snapshot, no new worker, and an egui pointer menu result. Final-source full suite: 2777 passed / 3 ignored; final Clippy/default check/build and formatting passed. Native menu/clipboard/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-plain-byte-copy-2026-10-30/README.md).

## Hex repeat-search shortcuts (2026-10-30)

Independent adaptation of [Double Commander viewer Find next/previous](https://doublecmd.github.io/doc/en/viewer.html), cohort #3, following Commander's existing preview keyboard conventions. Focused hex query accepts F3/Enter forward and Shift+F3/Shift+Enter backward, retains focus and reuses the deferred search owner. A loaded page/no-worker guard prevents duplicate work; other focus does not trigger search. Two headless keyboard/worker regressions cover all keys, directions, wrap, focus retention and busy suppression. Full suite: 2779 passed / 3 ignored; final Clippy/default check/build and formatting passed. Native keyboard/focus validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-search-shortcuts-2026-10-30/README.md).

## Hex mixed literal/byte search (2026-10-30)

Independent adaptation of [Midnight Commander mixed quoted-text/hex search](https://source.midnight-commander.org/man/mc.html), cohort #40. Mixed text/hex combines exact UTF-8 literals and hex byte constants with optional 0x/leading zeroes and explicit bounded escapes. Strict whitespace/token/value rules reject incomplete or malformed patterns before work; input/output caps are 1024 characters/256 bytes. Existing HEX syntax stays unchanged. Two parser regressions cover the upstream example, Unicode/controls/escapes, strict errors and budgets; cyclic worker coverage now includes mixed patterns across chunk boundaries and both wrapped directions. Full suite: 2781 passed / 3 ignored; Clippy/default check/build and formatting passed. Native mode/query/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-mixed-search-2026-10-30/README.md).

## Hex relative offsets (2026-10-30)

Independent adaptation of [Far Manager ViewerGotoPos](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Go accepts signed decimal/0x/whole-percent steps from the loaded page base; unsigned positions stay absolute. Exact checked arithmetic rejects overflow, before-start and beyond-EOF relative destinations before work and preserves the page on invalid input. Two regressions cover syntax/u64/percent/EOF boundaries and actual deterministic page reads/bytes with invalid-input no-op. Existing worker identity/cancel owners remain. Full suite: 2783 passed / 3 ignored; Clippy/default check/build and formatting passed. Native Go/Enter/error validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-relative-offset-2026-10-30/README.md).

## Hex explicit radix offsets (2026-10-30)

Independent adaptation of [Far Manager ViewerGotoPos radix notation](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Go accepts $FF/FFh hex and 255m decimal, including signed relative forms. Compatible hex markers combine; conflicting/empty/internal-sign/overflow forms fail. Bare digits stay decimal; whole percentages retain decimal-only grammar and m is not a size unit. Two regressions cover syntax/extrema and actual deterministic worker bytes/bounds across pages. Final-source full suite: 2785 passed / 3 ignored; Clippy/default check/build and formatting passed. Native entry/hover/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-explicit-radix-offset-2026-10-30/README.md).

## Hex successful-position history (2026-10-30)

Independent adaptation of [Far Manager viewer position undo](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Back/Forward retain up to 64 saved byte offsets across Go/page/bookmark/search hits, committing traversal only after guarded page acceptance. New successful branches clear Forward; duplicate positions, failed/cancelled reads do not grow/consume history. Reload starts a new history; current settings/bookmarks are not restored from snapshots. Three regressions cover bounded stacks/branches and real worker cancellation/failure/reset/search navigation. Full suite: 2788 passed / 3 ignored; Clippy/default check/build and formatting passed. Native click/recovery/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-position-history-2026-10-30/README.md).

## Hex Go/Search focus commands (2026-10-30)

Independent adaptation of [Midnight Commander viewer F5 Goto / F7 Search](https://github.com/MidnightCommander/mc/blob/master/doc/man/mc.1.in), cohort #40. Unmodified F5/F7 focus existing offset/query fields and request scroll reveal without starting work or changing values/page. Modified keys and busy periods do not activate these focus actions. Stable field IDs and header/hover hints make the interaction explicit. Two headless input/worker regressions cover focus, retained state, modifiers and busy suppression. Full suite: 2790 passed / 3 ignored; Clippy/default check/build and formatting passed. Native function-key/focus/scroll validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-focus-shortcuts-2026-10-30/README.md).

## Hex legacy Cyrillic encodings (2026-10-30)

Independent extension of [Far Manager codepage menu](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. More encodings adds strict KOI8-R/KOI8-U/IBM866 exact-byte search, sharing conversion/caps/error policy and existing directional/cyclic worker guards. A shared mode-change owner resets stale matches while retaining query/page/history and rejects changes while busy. Independent Python-checked byte vectors and a parser regression cover Cyrillic/Ukrainian/DOS differences, limits and no-loss encoding; existing cyclic/error worker tests now cover all new modes. A headless menu pointer regression verifies KOI8-U selection/state retention. Final-source full suite: 2792 passed / 3 ignored; Clippy/default check/build and formatting passed. Native menu/search/layout validation remains pending while CUA is unavailable. [Qualification](../qualification/hex-legacy-cyrillic-2026-10-30/README.md).

## Hex recent searches (2026-10-30)

Independent adaptation of [Far Manager dialog input history](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Recent searches stores up to 20 submitted query/mode/wrap snapshots with a 16 KiB raw-text cap and exact deduplication. Recall restores settings without starting work, clears stale matches and retains page/position history; clear affects only recent entries. History is per-viewer memory, retained across Reload and discarded on close. Three regressions cover bounds/deduplication, worker admission/error and no-work recall, and an egui pointer selection restoring KOI8-U/wrap. Final suite: 2795 passed / 3 ignored; Clippy/default check/build and formatting passed. Fresh extended native HexSearch passed 14/14 with zero effects, including recent recall/resumed search. Visual inspection caught and corrected a vertical More encodings trigger despite initially green generic checks; rejected and corrected frames are retained. Other native gates remain open. [Qualification](../qualification/hex-recent-searches-2026-10-30/README.md).

## Archive export native qualification and bounded sheet (2026-10-30)

Continues the Double Commander-inspired archive name/path/CSV export (cohort #3). Fresh native validation found and fixed an overflowing archive pane: the full sheet now scrolls within its available height, the member list retains a finite virtualized viewport, and extraction actions wrap. After waiting for initial layout settlement, ArchiveExport and ArchiveBrowser each passed 14/14; five exact recording clipboard writes and no other effects in Export, zero effects in Browser. Both frames inspected, including visible lower extraction actions after scrolling. Other archive workflows and OS pasteboard remain separate gates. [Qualification](../qualification/archive-native-validation-2026-10-30/README.md).

## Hex Go expression history (2026-10-30)

Independent adaptation of [Far Manager dialog edit-control history](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Recent offsets keeps 20 deduplicated, bounded, submitted Go expressions; recall restores/focuses the input without reading or changing loaded-page/search/bookmark/position-history state. Relative expressions are evaluated again only on Go. Invalid/busy inputs are excluded, worker-rejected admitted inputs remain, Reload retains history and closing discards it. Two real-worker/egui-pointer regressions cover bounds, admission, recall and actual Go reevaluation. Full suite passed 2797 / 3 ignored. Native HexPreview menu/recall/Go extension is prepared; execution remains pending because CUA is not exposed in this continuation. [Qualification](../qualification/hex-recent-offsets-2026-10-30/README.md).

## Archive viewport-sized cursor moves (2026-10-30)

Independent adaptation of [Yazi viewport-height cursor movement](https://yazi-rs.github.io/docs/configuration/keymap/#arrow), cohort #27. PageUp/PageDown in the archive filter move by the bounded member-list viewport using the actual row pitch; Shift reuses additive range selection, skips synthetic rows and preserves hidden selection. Moves clamp at list edges, request cursor reveal and retain existing hidden-cursor recovery. The shared input owner checks filter focus and leaves Ctrl/Alt/Command keys untouched. Two new input/core regressions pass; full suite 2799 passed / 3 ignored. Native key delivery, nested scroll reveal and Shift selection remain pending because CUA is unavailable in this continuation. [Qualification](../qualification/archive-page-navigation-2026-10-30/README.md).

## Archive names without extension and parent-folder copy (2026-10-30)

Independent adaptation of [Yazi copy name_without_ext / dirpath](https://yazi-rs.github.io/docs/configuration/keymap/#copy), cohort #27. Archive Copy visible/selected now offers lexical final-suffix removal and distinct internal parent folders (root `.`), using current sorted real-member IDs and retaining hidden selections. Strict UTF-8, control escaping, no partial output and 4 MiB bounds remain shared. Three formatter/row-owner regressions cover lexical cases, duplicate identities, parent order/deduplication/root, unsafe output and hidden/synthetic membership. Full suite passed 2802 / 3 ignored. Native selection/layout/clipboard gates remain pending while CUA is not exposed. [Qualification](../qualification/archive-extra-copy-formats-2026-10-30/README.md).

## Saved archive query/settings snapshots (2026-10-30)

Independent adaptation of [Far Manager dialog input history/frequent strings](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Save filter stores query plus Regex/case/exclusion/type settings, bounded to 20 saves / 16 KiB query text, with exact full-key deduplication and bounded parser validation. Saved filters restores settings in the current folder/narrowing while retaining sorting and selection owners. Reset retains snapshots; clear removes only saved entries; new/reloaded/closed archive discards them. Three core/input regressions pass, including real egui pointer recall; full suite 2805 passed / 3 ignored. Native Save/menu/clear/focus/layout gates remain open while CUA is not exposed. [Qualification](../qualification/archive-saved-filters-2026-10-30/README.md).

## Remove one saved archive filter (2026-10-30)

Independent adaptation of [Far Manager individual dialog-history deletion](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Each saved archive filter has a Remove action; deletion matches its complete query/settings snapshot and preserves the active filter, cache, cursor/range and selection owners. Same-query snapshots with different settings remain distinct. Pointer regressions caught rejected nested-context and row-layout implementations; the final bounded horizontal row passes both removal without recall and existing recall. Two new regressions pass; full suite 2807 passed / 3 ignored. Native action/layout/focus gates remain open while CUA is not exposed. [Qualification](../qualification/archive-remove-saved-filter-2026-10-30/README.md).

## Individual hex input-history removal (2026-10-30)

Independent adaptation of [Far Manager individual dialog-history deletion](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Recent searches and Recent offsets now offer per-entry Remove, matching complete search query/mode/wrap snapshots or exact Go expressions. Both preserve active page/input/matches/status/error, bookmarks and position history and queue no work. A shared bounded row separates recall and Remove; busy guards cover open-popup actions as well as openers. Two new pointer regressions and all 14 pointer tests pass, including previous recall/Go flows. Full suite: 2809 passed / 3 ignored. Native menu/action/layout/focus gates remain pending while CUA is not exposed. [Qualification](../qualification/hex-remove-history-entry-2026-10-30/README.md).

## Archive declared-size constraints (2026-10-30)

Independent adaptation of [fd inclusive size predicates](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. File size accepts exact/minimum/maximum or two intersected constraints with decimal/binary units and checked u64 parsing, including zero and values above 2^53. Active constraints filter declared unpacked regular-file sizes, preserving valid synthetic navigation and hidden selections; invalid syntax fails closed with an error. The separately cached predicate composes with path/type/folder/narrowing. Saved filters include size text in snapshot identity and the retained-text cap; unified Reset clears size input, including invalid Unicode-whitespace cases. Two pipeline regressions cover arithmetic, bounds, metadata types, navigation, hidden selection and saved/Reset recovery. Final suite passed 2811 / 3 ignored; Clippy/default check/build/formatting passed. Native input/error/recall/layout gates remain open while CUA is not exposed. [Qualification](../qualification/archive-size-filter-2026-10-30/README.md).

## Configurable hex bytes per row (2026-10-30)

Independent adaptation of [Far Manager hex row-width adjustment](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Bytes per row selects 1–64 with 8/16/32 presets; default remains 16. Display and Copy page share width/radix/ASCII/padding, absolute offsets and eight-byte separators. Width changes reset the inner byte-scroll identity and search-hit row calculations use the selected width. No extra I/O or history/bookmark/query mutation; Reload retains width, a new viewer resets it. Two regressions round-trip every byte and offset at all 64 widths in both radices and click preset/Copy page with a golden dump/no-work assertion. Full suite: 2813 passed / 3 ignored. Native numeric entry/preset/copy/horizontal-scroll/highlight/layout gates remain open while CUA is not exposed. [Qualification](../qualification/hex-row-width-2026-10-30/README.md).

## Archive filename-only filter scope (2026-10-30)

Independent adaptation of [fd filename vs full-path matching](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Filename only projects real/synthetic candidates to their final component for literal/regex matching and exclusion, retaining full-path default and original displayed/exported paths. Cached lowercase slices avoid per-member projection allocation; Options identity invalidates the existing visible cache and carries scope through saved snapshots, removal identity and Reset. Two parser/pipeline regressions cover anchors/case/inline flags/Unicode, duplicate IDs, navigation, exclusion, hidden selections and saved restoration. Full suite: 2815 passed / 3 ignored. Native checkbox/entry/navigation/recall/focus/layout remains open while CUA is not exposed. [Qualification](../qualification/archive-filename-scope-2026-10-30/README.md).

## Exact literal archive matching (2026-10-30)

Independent adaptation of [fd exact matching](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Exact compares the entire chosen path/name literally with preserved edge spaces, shared case policy and no regex interpretation. Regex/Exact toggle each other off in the UI and conflicting compiler flags fail closed. Scope/exclusion/size/type/folder/narrowing remain composable; Options identity carries Exact through cache invalidation, saved snapshot deduplication/recall/removal and Reset. Two parser/pipeline regressions pass; full suite 2817 passed / 3 ignored. Native mode exclusivity/entry/recall/focus/layout remains open while CUA is not exposed. [Qualification](../qualification/archive-exact-filter-2026-10-30/README.md).

## Archive glob mode (2026-10-30)

Independent adaptation of [fd glob matching](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77, using locked globset 0.4.21 (default features off). Glob supports whole-candidate wildcards, classes, alternatives and escapes; ordinary wildcards stay within full-path components, **/ crosses directories, and Filename only selects basename scope. Globset byte-oriented ?/class semantics are explicit in UI/README; Unicode literals share the cached lowercase case policy. Compiler/UI enforce exclusivity with Regex/Exact. Saved/Reset/cache/exclusion/navigation/hidden-selection owners remain shared. Two parser/pipeline regressions pass; full suite 2819 passed / 3 ignored. Native mode/entry/error/recall/focus/layout remains pending while CUA is not exposed. [Qualification](../qualification/archive-glob-filter-2026-10-30/README.md).

## Archive executable regular-file constraint (2026-10-30)

Independent adaptation of [fd executable-file filtering](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Types offers Executable files only, intersecting existing type choices with known declared executable status on regular files; unknown/false status, directories, symlinks and other records fail. No extension guessing, host access test, payload read or execution. Kinds identity carries the flag through visible-cache rebuild and saved/Reset/Show all types, while synthetic navigation and hidden selections remain shared. Two model/pipeline regressions pass; full suite 2821 passed / 3 ignored. Native Types/recall/reset/navigation/focus/layout remains open while CUA is not exposed. [Qualification](../qualification/archive-executable-filter-2026-10-30/README.md).

## Archive-root depth filtering (2026-10-30)

Independent adaptation of [fd min/max/exact depth](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Depth accepts exact, minimum, maximum and inclusive ranges of archive-root path levels, with bounded checked u32 parsing and independent caching. Valid constraints filter real member records while retaining synthetic navigation; invalid constraints fail closed for both. Depth stays archive-root-relative after folder navigation, composes with existing predicates and preserves hidden selection. Saved snapshot identity/text cap/labels/recall and unified Reset include depth. Two model/pipeline regressions pass; final full suite 2823 passed / 3 ignored after a test-fixture style correction. Native entry/error/recall/focus/layout remains open while CUA is not exposed. [Qualification](../qualification/archive-depth-filter-2026-10-30/README.md).

## Archive UTC modification-date filtering (2026-10-30)

Independent adaptation of [fd modification-time constraints](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Modified (UTC) accepts a day or open/closed date range with inclusive start and exclusive ending midnight, known timestamp requirements and fixed UTC semantics. Cached parsing composes with existing predicates, retains valid navigation and hidden selection, fails closed on invalid input, and joins saved identity/text caps/recall/Reset. Two parser/pipeline regressions cover nanosecond endpoints, pre-epoch/known-zero/missing values, leap/invalid dates, cache/navigation/selection and saved recovery. Full suite 2825 passed / 3 ignored; Clippy/check/build/formatting passed. Relative ages and RFC3339/local-time input are outside this transfer; native entry/error/recall/focus/layout remain pending while CUA is not exposed. [Qualification](../qualification/archive-modified-filter-2026-10-30/README.md).

## Archive same-name and same-parent selection (2026-10-30)

Independent adaptation of [Double Commander MarkCurrentName / MarkCurrentPath and inverse commands](https://doublecmd.github.io/doc/en/cmds.html), cohort #3. Same name selects/deselects visible regular files sharing the focused final-component stem, ignoring case; Same folder uses the exact case-sensitive internal parent of a visible real record and includes visible real record types. Both preserve hidden selection, skip synthetic IDs and avoid subtree expansion; application resets the range anchor without changing the view or saved selection. Three core/pointer regressions cover stems/dotfiles/duplicates, parent case/root/type semantics and actual Select/Deselect in both menus with hidden choices retained. Full suite 2828 passed / 3 ignored; Clippy/default check passed. Native menu/action/focus/layout remains open while CUA is not exposed. [Qualification](../qualification/archive-similar-selection-2026-10-30/README.md).

## Archive independent selection masks (2026-10-30)

Independent adaptation of [Double Commander MarkPlus / MarkMinus](https://doublecmd.github.io/doc/en/cmds.html), cohort #3. A collapsed Select by mask section adds/removes visible real member matches using an independently cached glob/case/path scope, preserving the view and hidden/saved selections. Blank/invalid/overlong masks and cache-length mismatch fail before mutation; navigation IDs are skipped and duplicate members remain independent. Successful commands reset the range anchor; mask settings belong to the loaded archive and are separate from saved/reset view filters. Three model/input regressions cover case/scopes/types/duplicates, no-partial-error behavior and actual pointer Select/Deselect with hidden selection retained. Full suite 2831 passed / 3 ignored. Native collapse/input/actions/focus/layout remains open while CUA is not exposed. [Qualification](../qualification/archive-selection-mask-2026-10-30/README.md).

## Hex loaded-page percentage status (2026-10-30)

Independent adaptation of [Double Commander viewer position percentages](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Hex status now shows the loaded page's start relative to its snapshot size, floored to 0.01% with u128 integer arithmetic. Empty files show n/a, EOF 100.00%, and earlier positions never round up to EOF even near u64::MAX. The label distinguishes position from read/search progress; no worker or history changes are added. Two regressions cover boundary/large integer values and exact rendered status after a real deterministic page read without new work. All ten focused position tests and the full suite (2833 passed / 3 ignored) pass; Clippy passes. Native visibility/wrapping/EOF/empty presentation remains open while CUA is not exposed. [Qualification](../qualification/hex-position-percentage-2026-10-30/README.md).

## Hex whole-file first/last search (2026-10-30)

Independent adaptation of [Double Commander boundary search directions](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Find first/last explicitly restart from the beginning/end of the bound file regardless of page or prior hit; ordinary next/previous continue from the resulting match. Both share the existing bounded, cancellable, identity-checked worker and query history. Three worker/pointer regressions cover both wrap settings, repeated boundary commands, continuation, busy/invalid refusal, empty files and actual button clicks. Full suite 2836 passed / 3 ignored; Clippy and default check passed. Native accessibility/focus/layout/cancel presentation remains pending while CUA is unavailable. [Qualification](../qualification/hex-boundary-search-2026-10-30/README.md).

## Archive literal extension alternatives (2026-10-30)

Independent adaptation of [fd repeated extension constraints](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Extensions accepts space-separated, case-insensitive literal suffix alternatives including optional leading dots, multi-part suffixes and dotfiles. The independently cached parser has 256-byte/32-token bounds and uses cached lowercase final filenames without member I/O. It intersects real-member predicates/types, preserves valid navigation and hidden selection, fails closed on invalid input and joins saved identity/text cap/labels/recall/Reset. Two regressions cover suffix/error boundaries, duplicate members, composition, navigation and saved recovery. Full suite passed 2838 / 3 ignored. Native field entry/error/recall/focus/layout remains pending while CUA is unavailable. [Qualification](../qualification/archive-extensions-filter-2026-10-30/README.md).

## Hex numbered bookmark keyboard commands (2026-10-30)

Independent adaptation of [Far Manager viewer bookmark shortcuts](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Ctrl+Shift+1–9 saves existing slots 1–9 and Ctrl+Shift+0 saves slot 10; Ctrl+digit jumps through the existing snapshot-bound page reader and history owner. Empty slots, busy work, open popups and Alt/Command combinations are suppressed; error pages cannot replace bookmarks. Hover hints explain the mapping. Two headless input/worker regressions cover every slot's bytes/history, query retention and guard behavior. Full suite passed 2840 / 3 ignored. Native macOS key delivery/focus/menu/hint gates remain pending while CUA is unavailable. [Qualification](../qualification/hex-bookmark-shortcuts-2026-10-30/README.md).

## Archive saved-filter menu search (2026-10-30)

Independent adaptation of [Far Manager menu/dropdown filtering](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Search saved filters matches a trimmed case-insensitive substring against complete preset descriptions, bounded to 128 characters and separate from active/saved archive predicates. Counts and Clear search expose filtered state; Clear all saved filters explicitly includes hidden snapshots. Recall/removal retain original snapshot identity. CloseOnClickOutside fixes field focus/dismissal; explicit action closure remains. A Unicode pointer regression covers input, unchanged active state and original-index-1 recall/removal; all 44 focused tests and the full suite (2841 passed / 3 ignored) pass. Native input/focus/menu/layout remains pending while CUA is unavailable. [Qualification](../qualification/archive-saved-filter-search-2026-10-30/README.md).

## Native qualification of recent hex transfers (2026-10-30)

Fresh CUA-launched extended HexSearch reached whole-file Find last/first at 120001/110001, injected Ctrl+Shift+0 slot-10 save and Ctrl+0 return, then the 8-byte preset. The final capture passed 14/14 native checks with zero recorded effects; its framebuffer was inspected for buttons, slot/offset, row layout and 83.90% status. The historical wrapped-result assertion now runs at its original phase 84, before later navigation clears the message; final phase 102 checks the persistent wrap setting. Failed stale-terminal-check artifacts are retained. This qualifies native rendering and scenario events, leaving physical OS key delivery, other widths/slots, copy/highlight and EOF/empty/cancel gates open. [Qualification](../qualification/hex-native-boundaries-bookmarks-2026-10-30/README.md).

## Hex keyboard row-width adjustment (2026-10-30)

Independent adaptation of [Far Manager Alt/Control+Alt row-width keys](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Alt+Left/Right adjusts one byte; Ctrl+Alt+Left/Right selects the next lower/higher 16-byte boundary within 1–64. Focused widgets, popups, busy work and unsupported modifiers suppress commands. Two input/worker regressions cover extrema/boundaries and no-read/state preservation; all four row-width tests pass. Fresh extended native HexSearch verified 8 -> 9 -> 16 -> 32 -> 31 -> 16 before its 14/14 capture with zero effects. Native QA now isolates input stages per frame and waits for stable pointer targets; rejected pre-key phase-54/36 captures are retained. Physical OS key delivery, VoiceOver and remaining manual/copy/focus cases remain open. [Qualification](../qualification/hex-row-width-shortcuts-2026-10-30/README.md).

## Archive hidden-component visibility filter (2026-10-30)

Independent adaptation of [fd hidden entry exclusion](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Hide dotfiles excludes any dot-prefixed archive-path component and descendants for real members and synthetic navigation, using archive names without host metadata or link traversal. Default visibility stays all; saved identity/description/recall/Reset/cache and predicate intersection share existing owners. Hidden selected IDs stay selected and selected-only copy keeps them. Show all types preserves visibility. Two model/pointer regressions pass. Fresh CUA-launched ArchiveBrowser toggled the actual control and verified hidden member/navigation removal with hidden selection retained; 14/14 checks, inspected framebuffer and zero recorded effects. Native saved/Reset, physical keys, VoiceOver and broader formats remain open. [Qualification](../qualification/archive-dotfile-filter-2026-10-30/README.md).

## Archive independent exclusion glob (2026-10-30)

Independent adaptation of [fd --exclude](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Exclude glob composes with the main query, supports case-insensitive filename/ancestor or Full paths prefix matching and directory-descendant exclusion. It shares the bounded glob compiler (brace alternatives, byte-oriented ?), independently cached settings, fail-closed errors, saved identity/text cap/description/recall and Reset. Hidden selected IDs remain selected. Two model/pointer regressions cover composition, scopes, ancestors/navigation, duplicates, errors and saved recovery. Fresh CUA-launched ArchiveBrowser typed *2.rs, verified file2.rs omitted and prior hidden/selection invariants retained, then passed 14/14 capture checks with zero effects; its framebuffer was inspected. Natural-order proof remains before the exclusion is entered. Native Full paths/saved/Reset/error/ancestor breadth, physical input and VoiceOver remain open. [Qualification](../qualification/archive-exclusion-glob-2026-10-30/README.md).

## Native archive metadata preset round trip (2026-10-30)

Fresh CUA-launched ArchiveBrowser typed size +1b, depth 1..3, Modified UTC 1970-01-01 and extension rs alongside prior dotfile/exclusion inputs, then used actual Save/Reset/Saved filters/recall controls. A required intermediate gate proves clear active settings and one retained preset; final phase 35 requires exact metadata, full snapshot equality, previous filtering/sorting/location and hidden-selection invariants. Fresh native capture passed 14/14 checks with zero effects and its restored-value framebuffer was inspected. Snapshot construction now shares one owner between saving and read-only QA comparison. This closes the concrete native preset round trip; physical input, VoiceOver, invalid/alternate inputs, multiple presets and menu search/removal remain open. [Qualification](../qualification/archive-filter-save-reset-native-2026-10-30/README.md).

## Hex position-history keyboard commands (2026-10-30)

Independent adaptation of [Far Manager position undo](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Ctrl+Z and Alt+Backspace use existing Back; Ctrl+Shift+Z is Commander's Forward companion. No focused widget, popup or busy work is allowed; text editing and Command combinations retain ownership. All transitions share the guarded history/page reader. An input/worker regression covers exact bytes, Back/Back/Forward, duplicate busy refusal and focus/popup/Command guards. Fresh native HexSearch drove Ctrl+Z 110001 -> 120001 and Ctrl+Shift+Z back, then passed 14/14 checks with zero effects and inspected retained layout/query/bookmark state. Physical OS delivery, native Alt+Backspace and focus/error/cancel variants remain open. [Qualification](../qualification/hex-history-shortcuts-2026-10-30/README.md).

## Hex page/file-boundary keys and input ownership (2026-10-31)

Independent adaptation of [Far Manager viewer navigation](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. PageUp/PageDown use Commander's 4 KiB read pages; Home/End and Control aliases use shared first/last-block semantics. Focus/menu/busy/modifier guards and no-op edges preserve editing and avoid redundant reads. Two real-worker regressions cover unaligned/exact/empty/tail bytes and history/bookmark/guard behavior. Native testing exposed and fixed file-panel key leakage before viewer rendering, with explicit focus transfer on open and a final unchanged-pane-cursor gate. Fresh native sequence 110001 -> 0 -> 4096 -> 0 -> 131072 passed 14/14 capture checks; inspected last-block framebuffer and zero effects are retained. Physical input, VoiceOver and remaining native variants stay open. [Qualification](../qualification/hex-page-navigation-keys-2026-10-31/README.md).

## Hex focus-key integration and repeat-search focus recovery (2026-10-31)

Qualification of the previously transferred Midnight Commander-style F5 Go/F7 Search found real pane-key leakage: F5 reached Copy before the viewer. Exact shared focus-key ownership now precedes pane dispatch, with busy/popup focus guards. The native Copy-dialog stack also exposed a nested context-lock timestamp initializer, now read before data mutation and regression tested. A subsequent native focus failure exposed query TextEdit disabling; keyboard searches now restore focus after completion while preserving another owner and clearing on cancel. New clock, popup and disabled-frame/repeat-F3 regressions pass. Fresh CUA-launched HexSearch completed F5/F7/F3 and retained query focus/match, with no operation modal/copied fixture, 14/14 capture checks and zero effects; framebuffer inspected. Aborted stack/focus evidence is retained. Physical keys, VoiceOver, standalone native Copy animation and other routes remain open. [Qualification](../qualification/hex-focus-key-routing-2026-10-31/README.md).

## Hex close shortcuts (2026-10-31)

Adapted Esc/F10 viewer close from [Far Manager](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Exact unmodified keys belong to the viewer before panel dispatch; open popups retain precedence. Closing cancels owned work, clears stale widget focus and requests existing cursor-row refocus. Regressions exercise initial/queued-read cancellation and popup/modifier guards. Native close/focus and physical-key qualification remain pending. [Qualification](../qualification/hex-close-shortcuts-2026-10-31/README.md).

## Hex close native focus qualification (2026-10-31)

Fresh CUA-launched HexSearch retained all former search/bookmark/width/query-focus gates, then used actual menu pointer events and egui Escape/F10 events: Escape dismisses the menu without closing the viewer; F10 closes the viewer and restores keyboard focus to the unchanged bytes.bin pane row. Phase 153 capture passed 14/14 checks, zero native effects; framebuffer inspected. This closes the prior native F10/popup/refocus evidence gap; physical OS delivery, native Escape-to-close and busy variants remain open. [Evidence](../qualification/hex-close-native-2026-10-31/README.md).

## Hex byte alignment keys (2026-10-31)

Adapted [Far Manager Ctrl+Left/Right hex alignment](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Ctrl arrows shift the page start one byte with unchanged row width, through the existing snapshot-checked reader and position history. Zero/EOF, busy, focus, popup and modifier guards prevent invalid or duplicate work and panel-key leakage. [Qualification](../qualification/hex-byte-alignment-2026-10-31/README.md).

## Dedicated native hex alignment boundaries (2026-10-31)

Added independent HexAlignment native scenario with a 4097-byte fixture and required 0/1/2/1/0/zero-no-op/4096/4097/EOF-no-op/4096 transitions. Actual viewer key events run with the documented no-widget-focus precondition set explicitly by the harness. Fresh phase-27 capture passed 14/14 checks with zero effects; final 00 tail byte/frame inspected. Prior composite-search timeouts and initial pane-focus refusal are retained. Automatic opening focus, physical keys, VoiceOver and the composite HexSearch alignment chain remain open. [Evidence](../qualification/hex-alignment-native-2026-10-31/README.md).

## Hex navigation focus ownership integration (2026-10-31)

Follow-up to Far-style hex navigation/alignment transfers. Native command opening left pane-row focus, suppressing viewer keys. The application now releases remembered pane-row focus and cancels deferred pane refocus before dispatch while the viewer is present, preserving text/menu owners. A rendered list regression covers actual focus, deferred-request removal across four frames, other-owner preservation and right-pane release. Dedicated HexAlignment removes its harness focus-clearing setup to test the application path. [Evidence](../qualification/hex-pane-focus-ownership-2026-10-31/README.md).

## Copy hex search match (2026-10-31)

Adapted [Far Manager viewer search-selection copy](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37, to read-only found bytes: Copy match offers HEX/decimal/strict UTF-8 from the accepted page snapshot, without surrounding bytes or new reads. Match offset/prefix/busy/error guards and invalid UTF-8 refusal apply. Two real-worker/rendered-menu regressions cover cross-boundary data, replacement after snapshot, all export formats and invalid bindings. [Evidence](../qualification/hex-copy-match-2026-10-31/README.md).

## Clear hex search selection (2026-10-31)

Adapted [Far Manager viewer Ctrl+U unselect](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37, to search highlights. Copy match / Clear match and guarded Ctrl+U clear visual selection and disable export while retaining exact page/query/history/bookmarks and the next-search continuation. A real-worker input regression covers guards and advancing to the next result; existing rendered menu coverage now includes Clear match. [Evidence](../qualification/hex-clear-match-2026-10-31/README.md).

## Hex match keyboard copy (2026-10-31)

Adapted [Far Manager viewer Ctrl+C/Ctrl+Insert](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37, with Cmd+C and semantic egui Copy support. Selected found bytes copy as HEX through the existing snapshot slice; fields/popups/manual label selection retain ownership and duplicate event/key delivery creates one request. Two real-worker/egui regressions include actual mouse-drag text selection and busy/cleared/modifier guards. [Evidence](../qualification/hex-match-copy-keys-2026-10-31/README.md).

## Hex single-byte character column (2026-10-31)

Adapted [Far Manager encoding-aware hex column](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Characters offers ASCII, Windows-1251/1252, KOI8-R/U and IBM866, affecting only rendered/copied page characters. Raw bytes/offsets/search/export modes stay independent, with controls/undefined/invisible spacing sanitized to dots and bounded lazy lookup tables. Two regressions cover all bytes/modes/layouts and actual selector/Copy page pointer actions without I/O. [Evidence](../qualification/hex-character-encodings-2026-10-31/README.md).

## UTF-8 hex character column (2026-10-31)

Adapted [Far Manager UTF-8 hex rendering](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37: leading scalars and › continuation markers. Up to three same-snapshot context bytes around each page preserve sequences across rows/pages and mid-sequence jumps, without changing public page bytes/export/search semantics. Strict UTF-8 validation and control/formatting sanitation apply. Two real-worker/rendered-menu regressions cover cross-boundary emoji/Cyrillic, invalid sequences and Copy page. [Evidence](../qualification/hex-utf8-character-column-2026-10-31/README.md).

## UTF-16 hex character column (2026-10-31)

Adapted [Far Manager UTF-16 viewer rendering/alignment](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Characters offers explicit LE/BE, even absolute code-unit alignment, validated surrogate pairs and cross-row/page/mid-unit context. Commander uses leading scalar plus › per remaining byte to preserve its byte-position convention; it does not claim Far's code-unit-width layout. Two regressions cover both byte orders, malformed/odd EOF units, boundary emoji and selector/Copy page without I/O. [Evidence](../qualification/hex-utf16-character-column-2026-10-31/README.md).

## Hex search-repeat aliases (2026-10-31)

Adapted [Far Manager Shift+F7/Space and Alt+F7](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. No-focus repeat keys share the existing search worker/query/Wrap and continuation; fields/popups/busy work retain ownership, and aliases are reserved before pane dispatch. Two regressions cover directions, wrapping, exact bytes and preserved state plus guards/invalid query. [Evidence](../qualification/hex-repeat-search-keys-2026-10-31/README.md).

## Hex character encoding keyboard controls (2026-10-31)

Adapted [Far Manager F8 / Shift+F8](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Commander cycles its supported nine Characters modes and opens that menu, retaining query focus/text and busy work without new reads. Shared viewer ownership prevents pane Delete leakage even in field/menu/busy states. Two regressions plus all 80 hex-preview tests cover mode wraparound, focused fields, busy/modifier/popup states and keyboard-opened menu selection. [Evidence](../qualification/hex-character-keys-2026-10-31/README.md).

## Hex Unicode BOM automatic selection (2026-10-31)

Adapted [Far Manager Unicode BOM/default code-page handling](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37, to the hex character column. Auto detects UTF-8/UTF-16 LE/BE at accepted byte zero, otherwise ASCII; full UTF-32 signatures are excluded. Manual/F8 choices retain ownership through reads/reload. Cached Auto restore needs no I/O and keeps raw bytes/query/search/page history independent. Three regressions cover signatures, manual/busy/reload policy and actual cached Auto menu restore away from zero. [Evidence](../qualification/hex-unicode-bom-2026-10-31/README.md).

## Archive fuzzy path/name filtering (2026-10-31)

Adapted [fzf subsequence matching](https://github.com/junegunn/fzf#search-syntax), cohort #75, using Commander's existing Rust predicate. Optional exclusive Fuzzy mode composes with scope/case/exclusion/metadata filters, saved recipes and Reset, preserving archive sorting and hidden selected record identities. No fzf grammar/scoring parity is claimed. Matcher/pipeline and rendered exclusive-toggle regressions cover Unicode, duplicate IDs, navigation and saved state. [Evidence](../qualification/archive-fuzzy-filter-2026-10-31/README.md).

## BLAKE3 checksum lists (2026-10-31)

Continues [Double Commander's checksum-file workflow](https://doublecmd.github.io/doc/en/help.html), cohort #3. Explicit BLAKE3 selection now exports a [b3sum checkfile](https://github.com/BLAKE3-team/BLAKE3/blob/master/b3sum/src/main.rs) instead of silently using SHA-256. Two-space formatting, escaped paths, lossless-path refusal and completed-row filtering reuse the existing report owner; no new I/O. [Qualification](../qualification/blake3-checksum-list-2026-10-31/README.md).

## CSV/TSV headings and sortable columns (2026-10-31)

Independently adapts [Strata's first-record table headings](https://github.com/lgse/strata/blob/ac5ab4f22d86798a40d7ad30eb3528d56eecf137/src/services/table.rs) as an explicit optional mode. Adds Commander column sorting (natural case-sensitive text, ascending/descending/original) with stable source IDs and original row numbers, bounded to the existing preview. Source text and formula-like cells remain unchanged. Native table_preview is added to CI. [Qualification](../qualification/table-headings-sort-2026-10-31/README.md).

## CSV/TSV row filtering (2026-10-31)

Adapts [fzf interactive filtering](https://github.com/junegunn/fzf#search-syntax), cohort #75, to loaded CSV/TSV cells: literal/Fuzzy with case/Smart case, Clear/focus and matching counts. Optional headings remain visible; source record IDs/numbers and selected sorting are retained. Empty results are cached; no new reads or ranking grammar. [Qualification](../qualification/table-row-filter-2026-10-31/README.md).

## Copy visible CSV/TSV records (2026-10-31)

Adapts [fzf's selected-output workflow](https://github.com/junegunn/fzf#using-the-finder), cohort #75, to filtered table records. Copy visible CSV/TSV uses the current row order, optional original headings and lossless cell quoting via the existing Rust csv crate. Hidden/unloaded records and UI row numbers are omitted; no extra reads. Requests use the existing pane-to-application clipboard owner. [Qualification](../qualification/table-copy-records-2026-10-31/README.md).

## CRC32 comparison and SFV export (2026-10-31)

Adapts [Double Commander's CRC32/SFV checksums](https://doublecmd.github.io/doc/en/help.html), cohort #3. Native one-pass CRC-32/IEEE shares existing cancellation/snapshot guards, explicit eight-hex comparison, report and digest copy. SFV lists use completed absolute-path records, rejecting names the unescaped format cannot represent. Existing SHA/BLAKE3 output stays algorithm-specific. [Qualification](../qualification/crc32-sfv-2026-10-31/README.md).

## Portable relative checksum lists (2026-10-31)

Adapts [rclone's root-relative hashsum names](https://github.com/rclone/rclone/blob/master/fs/operations/operations.go), cohort #87, to existing SHA/BLAKE3/SFV exports. Optional Relative paths shows the common completed-record directory, uses ./ names and preserves nested paths/format escaping without extra reads. Cached base and algorithm/mode/new-operation state stay bound; invalid/outside/traversing paths fail without an absolute fallback. [Qualification](../qualification/relative-checksum-lists-2026-10-31/README.md).

## Duplicate keep-newest and bulk plan controls (2026-10-31)

Adapts [Double Commander's newest duplicate retention](https://doublecmd.github.io/doc/en/help.html), cohort #3. Keep newest prefers known mtimes and stable first-file ties. Keep all groups clears proposals; Apply policy restores the selected rule after manual choices. Invalid keeper IDs now skip removal instead of selecting every member. Existing scan/Trash ownership stays intact. [Qualification](../qualification/duplicate-newest-policy-2026-10-31/README.md).

## Duplicate CSV proposal export (2026-10-31)

Adapts [fclones machine-readable reports](https://github.com/pkolaczk/fclones/blob/main/README.md), cohort #85, as Copy CSV plan with Commander group/action/path/size schema. Includes keep/trash/skip records, lossless quoting and shared valid-keeper interpretation; no plan mutation or added reads. Existing application clipboard/error handling remains the side-effect owner. [Qualification](../qualification/duplicate-csv-plan-2026-10-31/README.md).

## Duplicate group sorting (2026-10-31)

Adapts [Czkawka/Krokiet result sorting](https://github.com/qarmin/czkawka/blob/master/instructions/Instruction_Krokiet.md), cohort #81, as Scan order / Potential savings / File size / Copies. Cached view indices preserve manual keeper/skip bindings; wide potential keys and stable scan ties apply. CSV follows the view and validates a full permutation; removal targets stay identity-bound. [Qualification](../qualification/duplicate-group-sort-2026-10-31/README.md).

## Duplicate rescan preferences (2026-10-31)

Follow-up to the transferred duplicate retention and result-sorting workflows: Rescan retains the selected keep policy and group sort, clears old manual keep/skip decisions immediately and rebuilds decisions/order from the new hashes. Fresh opening still uses shortest path / scan order. The existing listing supplies candidates and timestamps; this does not add directory enumeration. A real-file asynchronous regression changes same-size payloads, removes a former group, checks newest retention and then an empty result. [Scoped evidence](../qualification/duplicate-rescan-options-2026-10-31/README.md).

## MD5 checksum interoperability (2026-10-31)

Adapts [Double Commander's MD5 checksum calculation/comparison](https://doublecmd.github.io/doc/en/help.html#012), cohort #3. RustCrypto MD5 joins the existing single streaming pass and snapshot/cancellation guards; explicit 32-hex comparison, report/digest copy and GNU binary MD5 manifests support both absolute and portable relative names. SHA-256 remains default. Real-file vectors include million-byte input; macOS md5 and RHash independently verify output, moved-tree acceptance and changed-content refusal. Extended native Checksums reaches phase 103, 14/14 capture checks, thirteen exact recording clipboard writes with other effects zero. [Qualification](../qualification/md5-checksums-2026-10-31/README.md).

## Recursive duplicate search (2026-10-31)

Independently adapts [fclones recursive/non-recursive file selection](https://github.com/pkolaczk/fclones#features), cohort #85. Include subfolders traverses the captured root on the background worker, including hidden folders, without intentionally following symlinks; repeated observed file/directory identities are collapsed. Empty files stay excluded. Recursive Rescan re-enumerates and captures metadata afresh; default pane-listing mode remains available. Scope changes preserve keep policy/sort and rebuild choices. Read/hash failures now surface a partial-scan count. [Qualification](../qualification/recursive-duplicates-2026-10-31/README.md).

## Duplicate size limits (2026-10-31)

Independently adapts [fclones minimum/maximum file-size selection](https://github.com/pkolaczk/fclones#features), cohort #85. Min bytes / Max bytes provide inclusive exact-u64 bounds for listing and recursive scans before size grouping/hashing. Explicit Apply size limits retains policy/sort and rebuilds choices; drafts show separately from the applied caption, invalid/overflowing inputs cannot apply, Reset restores all nonempty files, and scope/rescan preserve applied limits. [Qualification](../qualification/duplicate-size-limits-2026-10-31/README.md).

## Verify checksum-list files (2026-10-31)

Independently adapts [Double Commander's Verify Checksum workflow](https://doublecmd.github.io/doc/en/help.html), cohort #3. New cursor-file palette command opens explicit-algorithm list verification for GNU SHA-256/384/512/MD5, b3sum and CRC32/SFV records. A bounded UTF-8 manifest is parsed before target reads; the workload hash lane resolves same-folder relative/absolute records, refuses traversal/symlinks, reports Match/Mismatch/Error per row, supports cancellation/rerun and copies a report. [Qualification](../qualification/checksum-list-verify-2026-10-31/README.md).

## Watcher link-scope correction (2026-10-31)

Reliability follow-up to recursive duplicate/tab transfers, consistent with [fd's explicit --follow option](https://github.com/sharkdp/fd#command-line-options), cohort #77. The actual recursive native fixture exposed polling link-cycle restart loops. Explicit notify configuration now disables child-link following on supporting native backends and polling; physical subtree changes and link-node creation remain observed. A real-backend regression fails with the previous configuration and passes in recursive/shallow modes after the fix. Fresh native duplicate_recursive retains 14/14 checks, with zero loop/restart log lines versus eight in the prior capture. [Qualification](../qualification/watcher-link-scope-2026-10-31/README.md).

## Disk-usage tree filtering with ancestor context (2026-10-31)

Independently adapts [broot's directory-tree filtering/navigation](https://github.com/Canop/broot#find-a-directory-then-cd-to-it), cohort #31. Tree mode now filters the loaded compressed directory rows by root-relative path, with literal/Fuzzy, Match case and Clear. Real ancestor rows stay as labelled context; original byte totals/order/path identities are retained. The view caches even empty results, is invalidated on snapshot publication and survives Map/Tree or Rescan. Left-aligned labels retain hierarchy indentation. Model and pointer/path regressions pass; fresh native mode/case/clear/rescan round trip captures 14/14 with effects zero. [Qualification](../qualification/disk-tree-filter-2026-10-31/README.md).

## Disk-usage JSON and visible CSV reports (2026-10-31)

Independently adapts [gdu's machine-readable analysis export](https://github.com/dundee/gdu#usage), cohort #84. Copy scan JSON serializes retained root totals/map items/compressed directories and incomplete-scan metadata, independently of filtering. Copy visible CSV preserves current row order, original paths/subtree totals, ancestor-context markers and filter/scan flags. Bounded writers refuse over-1-MiB output; invalid/lossy paths and stale projections fail instead of copying partial data. No extra scans; running/failed-worker states disable copying. Fresh native reports before/after filtering and Rescan pass 14/14 with five exact recorded clipboard writes; payload files are retained and independently parsed. [Qualification](../qualification/disk-usage-export-2026-10-31/README.md).

## Disk-usage percentage bars (2026-10-31)

Independently adapts [dust's size percentages and bars](https://github.com/bootandy/dust#overview), cohort #82. Tree rows show their share of the captured root or nearest represented parent, with a Root/Parent switch. Full-snapshot ancestor lookup is independent of the active filter; compressed chains use the represented ancestor rather than inventing a physical parent's measurement. Exact integer tenths avoid u64 conversion errors, zero denominators show an em dash, and only bar geometry clamps over-100-percent partial/inconsistent values. No scan or report schema changes. [Qualification](../qualification/disk-usage-shares-2026-10-31/README.md).

## Disk-usage branch collapse (2026-10-31)

Independently adapts [dust's collapsed-subtree display](https://github.com/bootandy/dust#usage), cohort #82, as interactive controls on retained compressed directory branches. Individual buttons, Collapse all and Expand all preserve raw measurements and navigation identity, avoid rescanning and retain path-based choices across Map/Tree and Rescan. Search temporarily reveals matches; clearing it restores collapsed branches. Visible CSV follows actual shown rows with an appended collapsed boolean; full JSON remains unfiltered. [Qualification](../qualification/disk-tree-collapse-2026-10-31/README.md).

## Automatic checksum-list format and algorithm detection (2026-10-31)

Extends the [Double Commander checksum-list verification workflow](https://doublecmd.github.io/doc/en/help.html), cohort #3, with interoperable tagged records defined by [GNU checksum output modes](https://www.gnu.org/software/coreutils/manual/html_node/cksum-output-modes.html). Auto is now the default: recognized list filenames disambiguate SHA-256/BLAKE3; unknown names require exactly one algorithm to parse the entire bounded list. BSD/GNU tags and compact OpenSSL records preserve names containing parentheses, delimiter-like text, Unicode and escaped control characters. Explicit selection remains available; filename conflicts, ambiguous layouts and mixed algorithms fail before target hashing. Results identify the actual algorithm. [Qualification](../qualification/checksum-list-auto-2026-10-31/README.md).

## Checksum verification result review (2026-10-31)

Extends the [Double Commander error-focused log/review workflow](https://doublecmd.github.io/doc/en/configuration.html), cohort #3, in the checksum-list tool. All/Problems/Mismatches/Errors/Matches filters combine with case-insensitive filename search over cached completed records. Full verification totals and cancellation context stay visible; preferences survive reruns and new reports invalidate the projection. Copy visible report preserves full/partial summary, active filter/query and original row order with a streaming 1 MiB cap; Copy verification report remains full scope. No new target reads or scans. [Qualification](../qualification/checksum-list-filter-2026-10-31/README.md).

## Favorite display names and direct quick-jump assignment (2026-10-31)

Independently adapts [Nautilus bookmark label editing](https://help.gnome.org/gnome-help/nautilus-bookmarks-edit.html), cohort #22. Favorites context menus now edit/save a display name and assign slots 1–9 directly. Rename validates a trimmed 1–256-character label without control characters, preserving the original path, row order and quick-jump binding. Slot assignment reuses the existing unique-slot ownership rule. Invalid/unknown/unchanged edits do not save, successful edits use the existing bookmark persistence owner. Folder data is not renamed. [Qualification](../qualification/favorite-labels-2026-10-31/README.md).

## Favorite navigation destinations (2026-10-31)

Independently adapts [Nautilus sidebar/folder tab navigation](https://help.gnome.org/gnome-help/files-browse.html), cohort #22, to the dual-pane workspace. Favorites context menus offer other-panel, foreground-tab and background-tab targets; middle-click requests a background tab. The workspace binds the original favorite path, keeps metadata unchanged and reuses existing tab capacity/lifecycle/modal/operation gates. Background and other-panel opens preserve the current source folder/filter/preview; foreground opening retains the previous tab's directory/filter through the normal tab owner. Successful opens reveal a covering pane tool only at the destination; background opens keep it. [Qualification](../qualification/favorite-destinations-2026-10-31/README.md).

## GTK/Nautilus bookmark text exchange (2026-10-31)

Independently implements the [GTK bookmark text format](https://raw.githubusercontent.com/GNOME/gtk/main/gtk/gtkbookmarksmanager.c), used by cohort #22 Nautilus. New palette commands review pasted local file-URI/label records and copy current Favorites in that format. Preview classifies existing, duplicate, unsupported and invalid rows, allows explicit subset selection, and appends only still-new paths after revalidation. Existing metadata/slots win; new entries have no slots. Import commits through the existing guarded store before installing the new live model; blocked/pre-commit failure leaves the old model. Text limits are 64 KiB/1000 input lines, editor height is bounded, and no imported path is probed or mounted. [Qualification](../qualification/bookmark-exchange-2026-10-31/README.md).

## Favorites fuzzy hotlist (2026-10-31)

Independently adapts [Double Commander Directory Hotlist](https://doublecmd.github.io/doc/en/help.html). `Open favorite folder` and omnibar `@` mode search every favorite in an opening snapshot by fuzzy label/path, prioritize labels, and display up to 100 ranked results with total counts. Four destinations bind to the opening panel; dispatch revalidates original path membership and existing tab guards. Cached lookup performs no filesystem probes; same-frame text/Enter waits for the fresh projection. [Qualification](../qualification/favorite-picker-2026-10-31/README.md).

## Favorites list ordering (2026-10-31)

Independently adapts [Double Commander hotlist entry sorting](https://doublecmd.github.io/doc/en/directoryhotlist.html#usage-sorting), cohort #4. Whole-list palette/context-menu operations sort by natural case-insensitive name, path, or assigned quick-jump slot; equal keys and unassigned slot rows remain stable. Candidate persistence precedes live replacement, unchanged order does not write, and slot/path/label metadata stays intact. [Qualification](../qualification/favorite-sort-2026-10-31/README.md) distinguishes headless sorting interaction from native existing-Favorites regression.

## Batch selected-folder Favorites (2026-10-31)

Independently adapts [Double Commander Hotlist section 3.5](https://doublecmd.github.io/doc/en/directoryhotlist.html), cohort #4. A palette command appends selected visible directory rows in pane order, skipping files and existing bookmarks while preserving metadata/slots. It uses the existing listing without probes, excludes hidden/foreign selections, validates the entire candidate and commits once before live replacement. Empty/repeated batches do not write. [Qualification and coverage limits](../qualification/bulk-favorites-2026-10-31/README.md).

## Command-template bundle exchange (2026-10-31)

Independently adapts [Strata action import/export](https://github.com/lgse/strata/blob/main/docs/custom-actions.md) as a distinct Commander JSON template format. Bounded paste/review, per-entry selection and append-only merge preserve existing raw-command metadata; export uses the live store. Exact-source/identity revalidation, save-before-install, unchecked-choice retention and no-op writes are covered. Import never executes or transfers trust; the existing Run owner remains authoritative. Strata TOML/script/runtime/menu compatibility remains open. [Qualification](../qualification/template-exchange-2026-10-31/README.md).

## Saved-command template discovery (2026-10-31)

Adapts [Strata library search and scope controls](https://github.com/lgse/strata/blob/main/docs/custom-actions.md) to saved templates. Name/command/extensions term search and All templates scope retain original indices/order for existing row actions. Content/selection/query/scope caching invalidates on edits and reordering. Search Enter is isolated from Run, while selection only fills the draft. [Qualification](../qualification/template-browser-2026-10-31/README.md) includes native search/scope/row/clear interaction and explicit execution boundaries.

## Whole-selection template matching (2026-10-31)

Adapts [Strata action matching](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#matching). An opt-in saved/draft policy requires every opening item to be a matching non-directory when extension filters are set; legacy behavior remains default. All templates may reveal a record but cannot bypass the actual Run gate. Policy survives save/update and Commander JSON v2 exchange, with v1 legacy import and strict-policy downgrade refusal. Cache invalidation includes directory classification. [Qualification](../qualification/template-whole-selection-2026-10-31/README.md) includes model/serialization/cache regressions and a native disabled-Run pointer check.

## Saved-command selection-count limits (2026-10-31)

Adapts [Strata action selection-size bounds](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#manifest). Inclusive minimum/optional maximum compose with existing matching, participate in suggestions and disable actual Run/Enter for invalid input counts, even through All templates. Saved metadata and JSON v3 retain bounds, with old-format compatibility and explicit downgrade refusal. [Qualification](../qualification/template-selection-count-2026-10-31/README.md) includes model/cache/serialization regressions and native count-only disabled-Run enforcement.

## Saved-command input kinds (2026-10-31)

Adapts [Strata action kind matching](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#matching). Any/Files/Folders constraints compose over every captured row with existing count/extension policies, affect applicability and cannot be bypassed through All templates. Draft/persistent metadata and JSON v4 preserve kind; legacy defaults, supported old envelopes and downgrade refusal are covered. Expanded template settings isolate implicit Enter from Run. [Qualification](../qualification/template-input-kind-2026-10-31/README.md) includes model/cache/serialization regressions and native mixed-file/folder rejection at the actual Run button.

## Sequential per-item shell commands (2026-10-31)

Adapts [Strata per-item execution/error policies](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#how-an-invocation-runs) to the existing shell owner. Whole selection remains default; two sequential per-item modes stop or continue on errors. Immutable one-path expansion, opening-directory binding, first-call preview, live counters, item-bound diagnostics and bounded aggregate output retain the complete run context. Shared cancellation prevents future calls and missed pre-publication cancellation. Mode persists through saved metadata/Commander JSON v5. [Qualification](../qualification/per-item-command-2026-10-31/README.md) includes real process/error/cancel regressions and native two-item execution; full manifest/argv/runtime/Jobs compatibility remains open.

## Per-template working folders (2026-10-31)

Adapts [Strata working-directory control](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#manifest) to captured Opening/Other pane/Single selected folder choices. Resolution leaves selection placeholders unchanged, fixes CWD for the batch, preserves both opening/target trust gates and fails unavailable targets without fallback. Policy persists in place and through Commander JSON v6, with old-envelope refusal. [Qualification](../qualification/command-working-directory-2026-10-31/README.md) includes root-denial/model/serialization/cache tests, real selected/other CWD processes and native two-invocation other-pane execution.

## Immutable command result reports (2026-10-31)

Adapts command-output review patterns from [Double Commander](https://doublecmd.github.io/doc/en/help.html) and [Strata custom actions](https://github.com/lgse/strata/blob/main/docs/custom-actions.md) into Copy stdout/stderr and Commander-specific text/JSON reports. Worker-owned snapshots preserve original template, requested CWD and inputs across draft edits; attempted expansions and batch status describe the retained result. Bounded 4 MiB serialization rejects overflow without partial clipboard writes. [Qualification](../qualification/command-result-report-2026-10-31/README.md).

## Searchable command output (2026-10-31)

Adapts [Double Commander viewer search and console-output viewing](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Completed stdout/stderr now shares Commander's Unicode literal and background Regex find engine with focus-scoped bidirectional navigation, matching options, underline/reveal and wrap controls. Modal-owned search lifetime cancels old jobs on close/new run; full stream/report copying remains independent. [Qualification](../qualification/command-output-find-2026-10-31/README.md).

## Logical-line navigation in command output (2026-10-31)

Extends [Double Commander viewer line-number/Go to line patterns](https://github.com/doublecmd/doublecmd/blob/master/src/fviewer.pas), cohort #3, to retained stdout/stderr using the existing logical-line owner. A separate gutter preserves exact selectable/exported stream text; wrap-independent counts and focused Enter jumps include empty/trailing display lines. Invalid input leaves the view in place. [Qualification](../qualification/command-output-lines-2026-10-31/README.md).

## Command output text magnification (2026-10-31)

Adapts [Double Commander viewer font-size controls](https://doublecmd.github.io/doc/en/configuration.html#ConfigFonts), cohort #3. Existing typography controls now provide caller-owned 50–200% output magnification, independent of pane/app zoom, with proportional gutter measurement/drawing and cache-aware reflow. Search/logical lines and exported reports retain original content. [Qualification](../qualification/command-output-text-size-2026-10-31/README.md).

## Synchronization plan review filters (2026-10-31)

Adapts [Double Commander synchronization Show filters](https://doublecmd.github.io/doc/en/syncdirs.html), cohort #3. Cached literal name search and eight review scopes expose planned/skipped/conflicting/one-sided/paired/metadata-matching/folder rows using original identities. Hidden planned counts make clear that Apply/All remain whole-plan operations; filtering never rewrites guards or plan ownership. The form now scrolls within pane height. [Qualification](../qualification/sync-plan-review-2026-10-31/README.md).

## Sync plan CSV/JSON clipboard export (2026-10-31)

Adapts [Double Commander synchronization column copying](https://doublecmd.github.io/doc/en/syncdirs.html), cohort #3, into explicit Full plan/Shown rows reports. Original row/path identity, edited directions and full/exported counts remain visible across filtering; bounded 1 MiB serialization and completed-plan gates prevent partial or mislabelled clipboard results. [Qualification](../qualification/sync-plan-export-2026-10-31/README.md). Reports are read-only retained-plan diagnostics, not replay/import or fresh guard validation.

## Sync direction edits on shown rows (2026-10-31)

Adapts [Double Commander synchronization direction commands](https://doublecmd.github.io/doc/en/syncdirs.html), cohort #3. Skip shown, Restore shown defaults and Reverse shown copies capture original filter targets before editing, preserve hidden rows and respect available source directions/conflicts. Read-only plan editing uses retained statuses, reports changed counts and returns before Apply. [Qualification](../qualification/sync-shown-actions-2026-10-31/README.md).

## Sort synchronization review rows (2026-10-31)

Adapts [Double Commander sortable comparison lists](https://doublecmd.github.io/doc/en/syncdirs.html), cohort #3, into Plan order/natural Name/Status/Direction projection sorting with Descending and reset. Original plan order/identities stay fixed for Apply and full export; shown exports retain current view order with original row numbers and sort metadata. [Qualification](../qualification/sync-review-sort-2026-10-31/README.md).

## Difference-block navigation in text diff (2026-10-31)

Adapts [Double Commander Compare by Contents navigation](https://doublecmd.github.io/doc/en/help.html#cm_CompareContents), cohort #3. Cached contiguous-change blocks, First/Previous/Next/Last, source-line positions and selected-block reveal/highlight extend the read-only unified diff. Focus-owned Alt shortcuts share the existing diff key owner, and row virtualization remains intact. [Qualification](../qualification/diff-hunk-navigation-2026-10-31/README.md). Editing/merge/patch parity remains open.

## Compact diff context and expandable equal stretches (2026-10-31)

Commander-specific extension of [Midnight Commander hunk-oriented diff review](https://github.com/MidnightCommander/mc/blob/master/doc/man/mc.1.in#L2830), cohort #40. Compact diff retains 0–50 context lines around original change blocks, merges overlapping windows and provides exact-count expandable equal stretches. Original IDs keep navigation/highlighting stable through projection changes, with virtualized fixed-height rows and no source rereads. [Qualification](../qualification/diff-compact-context-2026-10-31/README.md). This is not mcdiff renderer compatibility or a merge/edit feature.

## Dual source line numbers in text diff (2026-10-31)

Adapts [Midnight Commander's optional diff line numbers](https://github.com/MidnightCommander/mc/blob/master/doc/man/mc.1.in#L2863), cohort #40. Optional minus/plus source columns use cached original row coordinates, with blank absent operands and stable numbering through compact context and gap expansion. [Qualification](../qualification/diff-source-lines-2026-10-31/README.md).

## Find text in diff, including folded context (2026-10-31)

Independent adaptation of [Double Commander Differ search](https://doublecmd.github.io/doc/en/shortcuts.html), cohort #3. Cached bounded Unicode literal/whole-word/regex row search, cyclic Previous/Next/Enter, selected-row highlight/reveal and exact hidden-context expansion extend the immutable comparison. Difference navigation retains its own focus/highlight. [Qualification](../qualification/diff-text-find-2026-10-31/README.md).

## Keyboard search in the diff tool (2026-10-31)

Adapts [Double Commander Differ search shortcuts](https://doublecmd.github.io/doc/en/shortcuts.html), cohort #3: Cmd/Ctrl+F and F7 focus the retained query; F3 / Shift+F3 cycle row results through the early diff key owner. A real TextEdit regression found and fixed Enter losing focus before result navigation. [Qualification](../qualification/diff-find-shortcuts-2026-10-31/README.md).

## Go to an original source line in diff (2026-10-31)

Adapts [Double Commander Differ Go to line](https://doublecmd.github.io/doc/en/shortcuts.html), cohort #3. Signed operand selection, one-based source lookup, bounded input/explicit validation, Enter/Go and Cmd/Ctrl+G reveal/highlight the retained row, expanding hidden context as needed. [Qualification](../qualification/diff-source-goto-2026-10-31/README.md).

## Reload the retained text comparison (2026-10-31)

Adapts [Double Commander Differ Reload files](https://doublecmd.github.io/doc/en/shortcuts.html), cohort #3. Reload compared files / focused Cmd/Ctrl+R reread the original operand paths using existing caps, rebuild diff/coordinates/context/search, preserve view/search settings and reset stale navigation/expansions. Identical/error states retain Reload for recovery. [Qualification](../qualification/diff-reload-2026-10-31/README.md).

## Relative symbolic links into the other pane (2026-10-31)

Independent adaptation of [Yazi symlink --relative](https://yazi-rs.github.io/docs/configuration/keymap/#symlink), cohort #27. A separate command uses the existing link-operation guards, collision naming and notices, computing relative native path components between physical containing directories while preserving the source's final symlink entry. Real filesystem regressions qualify moving a shared tree, files/folders, collision preservation, symlinked destination and dangling source links. [Qualification and limits](../qualification/relative-symlink-2026-10-31/README.md).

## Follow the highlighted symbolic link (2026-10-31)

Independent adaptation of [Yazi follow](https://yazi-rs.github.io/docs/configuration/keymap/#follow), cohort #27. A cursor-only navigation command resolves the live symlink chain, reveals its target row, preserves pane history/inactive root and exposes hidden/filtered targets using existing listing/focus owners. Broken/loop/replaced-link errors leave navigation unchanged. [Qualification](../qualification/follow-symlink-2026-10-31/README.md).

## Copy symbolic-link targets without navigating (2026-10-31)

Commander-specific extension of the [Yazi follow](https://yazi-rs.github.io/docs/configuration/keymap/#follow) transfer: separate recorded-target and resolved-target clipboard actions reuse the cursor-only symlink predicate and existing CopyText owner. Dangling recorded targets remain usable; resolution/live-type/UTF-8 errors stage no copy. [Qualification](../qualification/copy-symlink-target-2026-10-31/README.md). This is not a claim of identical upstream clipboard commands.

## Native qualification of the symbolic-link workflow (2026-10-31)

The relative-link/target-copy/Follow workflow now runs through four real command-palette queries and Enter actions in a new CI scene. Native 14/14, exact recording clipboard values, collision/link/source readbacks and actual target-row focus passed after correcting the harness's modifier-state events. [Evidence and limits](../qualification/symlink-workflow-native-2026-10-31/README.md).

## Select visible symbolic links in panels (2026-10-31)

Independent adaptation of [fd symlink type filtering](https://github.com/sharkdp/fd/blob/master/doc/fd.1#L209), cohort #77, into Add/Remove/Invert visible symbolic-link selection. Captured lexical identities distinguish links from target kinds and hard links, with no restat or change to hidden/nonlink picks. Three regressions and an extended eight-invocation native palette workflow qualify the transfer. [Evidence and limits](../qualification/symlink-selection-2026-10-31/README.md).

## Cached symbolic-link details in Get Info (2026-10-31)

Follow-up to [fd detailed symlink targets](https://github.com/sharkdp/fd/blob/master/doc/fd.1#L134), cohort #77. Existing Get Info target reads now use the listing observation instead of UI-thread readlink; explicit exists/missing/unavailable states avoid classifying permission/loop errors as missing. Unknown target size is unavailable, and native inspection corrected inspector spacing/arrow glyphs. [Qualification](../qualification/symlink-info-cache-2026-10-31/README.md).

## Reliable Stop for captured/per-item custom commands (2026-10-31)

Closed a reproduced cancellation defect in the QtFM/lf-inspired command transfer: a shell could be gone while a same-group `sleep` held output open. Retained Unix leader ownership through WNOWAIT, repeated cancellation signaling and atomic reap/retirement passed 256/256 parallel repetitions after a 62/64 baseline, full suite and normal native command capture. [Evidence and limits](../qualification/command-stop-race-2026-10-31/README.md).

## Native Run/Stop delivery for per-item commands (2026-10-31)

Added a dedicated native `per_item_stop` CI scene for the existing QtFM/lf-inspired command transfer. Actual egui Run/Stop button phases cancel the first invocation, preserve both input files and leave the second invocation unstarted; independent result/report assertions and 16/16 native checks passed. This is UI regression qualification, not another transferred command. [Evidence and limits](../qualification/command-stop-native-2026-10-31/README.md).

## Diff comparison policy (2026-10-31)

Independently adapted [Double Commander's Ignore Blanks / Ignore Case comparison controls](https://doublecmd.github.io/doc/en/help.html#cm_CompareContents): optional ASCII space/tab-run equivalence and Unicode lowercase matching now compose over retained bounded sources, with immediate recomputation and policy-preserving Reload. Original text and line endings stay explicit; coordinate/search/context state is rebuilt. Full suite 3050 passed/3 ignored, six checks zero; native options 14/14 and existing navigation/reload 15/15. [Evidence and limits](../qualification/diff-comparison-options-2026-10-31/README.md).

## Character differences inside changed lines (2026-10-31)

Independent adaptation of [Double Commander's Line Differences](https://doublecmd.github.io/doc/en/help.html#cm_CompareContents): **Highlight changes within lines** uses cached, bounded Unicode-character LCS for ordinally paired changed rows, preserving original text and highlighting unpaired rows in full. Explicit fallback counts expose simplified spans. Full suite 3054 passed/3 ignored, six checks zero; native character spans 14/14 and existing navigation/reload 15/15 after a layout fix. [Evidence and limits](../qualification/diff-intraline-2026-10-31/README.md).

## Duplicate scan basename selection (2026-10-31)

Adapted [fclones group --name](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md), cohort #85: staged include/exclude basename globs with case policy, explicit Apply/Reset and cached validation now narrow candidates before hashing in both scopes. Identity collapse follows filtering so matching hard-link aliases survive excluded names. Full suite 3057 passed/3 ignored, six checks zero; native name workflow 14/14 and previous size workflow 14/14. [Evidence and limits](../qualification/duplicate-name-filters-2026-10-31/README.md).

## Duplicate scan depth boundary (2026-10-31)

Adapted [fclones recursion-depth selection](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md#finding-files), cohort #85: staged **Subfolder levels** with Apply/Reset bounds the background directory queue before deeper enumeration and hashing. Commander labels root files as level 0; this is explicitly distinct from fclones' CLI numbering. Size/name policies and identity/cancellation handling compose with the bound. Full suite 3059 passed/3 ignored, six checks zero; native depth workflow 14/14 and name-filter regression 14/14. [Evidence and limits](../qualification/duplicate-depth-limit-2026-10-31/README.md).

## Minimum verified duplicate copies (2026-10-31)

Adapted [fclones replica-count selection](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md#finding-files), cohort #85: staged inclusive **Groups with at least N copies** with Apply/Reset narrows both hash-eligible size buckets and fully verified digest groups. Scope/filter/rescan preferences retain the threshold; actual keeper policy stays separate. Full suite 3062 passed/3 ignored, six checks zero; native threshold, previous name and previous size workflows each 14/14. [Evidence and limits](../qualification/duplicate-minimum-copies-2026-10-31/README.md).

## Duplicate report with applied scan context (2026-10-31)

Adapted [fclones machine-readable JSON reporting](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md), cohort #85: **Copy JSON report** exports the retained proposal with applied scan settings, partial status, sorted/original group IDs and authoritative keep/trash/skip actions through a bounded writer. Shared CSV/JSON validation preserves safe skipping and exact paths; drafts and new filesystem reads are excluded. Full suite 3064 passed/3 ignored, six checks zero; native JSON and existing CSV workflows each 14/14. A discovered legacy listing-test race was corrected with the existing deterministic backend and passed 64/64 isolated repeats. [Evidence and limits](../qualification/duplicate-json-report-2026-10-31/README.md).

## Duplicate search across both panel folders (2026-10-31)

Adapted [fclones multi-root scanning](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md#finding-files), cohort #85: **Include other panel folder** captures both roots, scans their union with existing scope/depth/name/size/copy policies and disambiguates row labels. Shared identity/path filtering and shallower-overlap revisits preserve depth semantics. Returning to a captured primary folder after pane navigation uses fresh enumeration; JSON exposes combined context. Full suite 3067 passed/3 ignored, six checks zero; native two-root/context-binding and ordinary JSON workflows each 14/14. Arbitrary root-list UI and isolate-only matching remain open. [Evidence and limits](../qualification/duplicate-two-roots-2026-10-31/README.md).

## Duplicate groups spanning both folders (2026-10-31)

Independently adapted [fclones isolate selection](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md#finding-files), cohort #85: **Only groups spanning folders** prunes local-only sizes before hashing and selects fully verified groups with copies assigned to different captured roots. Most-specific overlap ownership is shared with row labels; physical-copy minimums and hard-link collapse remain explicit. Retained scope preference and optional JSON context are covered. Full suite 3069 passed/3 ignored; six checks zero; native crossing and previous union workflows each 14/14. This two-root policy does not implement full fclones hard-link isolate semantics. [Evidence and limits](../qualification/duplicate-cross-roots-2026-10-31/README.md).

## Reverse the retained text comparison (2026-10-31)

Independently adapted [Midnight Commander internal diff Ctrl+U](https://github.com/MidnightCommander/mc/blob/master/doc/man/mc.1.in), cohort #40. **Swap compared sides** and focus-owned Ctrl+U reverse captured operands and loaded snapshots, recomputing directional rows/coordinates while retaining comparison, search and view policies. Text fields retain Ctrl+U editing; repeats do not reverse again. Reload follows the reversed captured paths. Full suite 3072 passed/3 ignored; six checks zero; native swap/field-ownership/Reload and prior options workflows each 14/14. [Evidence and limits](../qualification/diff-swap-sides-2026-10-31/README.md).

## Search symbolic links with missing targets (2026-10-31)

Independently adapted [rmlint broken-link discovery](https://github.com/sahib/rmlint/blob/master/README.rst), cohort #86. `type:broken-link` / `type:broken-symlink` and `types:` alternatives use worker-observed target state, optional cached facts and explicit unknown handling. Missing/invalid path components qualify; permission/loop failures and unresolved ZIP/TAR targets do not become missing-target matches. Indexed queries preserve observations until rebuild; Find displays skipped coverage. Full suite 3077 passed/3 ignored; six checks zero; native real-query/repair-refresh workflow 16/16 and cached Info regression 14/14. [Evidence and limits](../qualification/broken-symlink-search-2026-10-31/README.md).

## Selected-file timestamp editing (2026-10-31)

Independently adapted [Double Commander Change Attributes timestamps](https://doublecmd.github.io/doc/en/help.html#cm_SetFileProperties), cohort #3. **Change file timestamps** provides modified/accessed drafts, explicit UTC/offset parsing, Now/keep controls, before/after preview and worker Apply/Stop/Reload over captured protected objects. Edited-field conflicts, replacement/link refusal, omitted unedited fields and hard-link idempotence are explicit. Full suite 3087 passed/3 ignored; six checks zero; timestamp native workflow 16/16 twice with exact filesystem facts and click delivery, previous permission preview 16/16. Recursive changes, creation time and Undo remain open. [Evidence and limits](../qualification/file-timestamps-2026-10-31/README.md).

## Synchronize paired files by content (2026-10-31)

Independently adapted [Double Commander Synchronize directories by content](https://doublecmd.github.io/doc/en/syncdirs.html), cohort #3. **Compare file contents** performs cancellable fresh byte reads: equal bytes default to Skip despite date differences, while different bytes override metadata equality. Content scopes and CSV/JSON facts expose equal/different/unverified rows; failed or unsafe comparisons cannot authorize copying. Captured lexical identities and mode bind the guarded plan. Full suite 3096 passed/3 ignored; six checks zero; native content and existing review workflows each 15/15. Real guarded-copy regression verifies destination bytes and preservation of an equal file's timestamp. Shallow scan-time proof does not provide recursive or atomic multi-file content synchronization. [Evidence and limits](../qualification/sync-content-comparison-2026-10-31/README.md).

## Ignore absent spacing in text comparisons (2026-10-31)

Independently adapted [Midnight Commander's Ignore all whitespace option](https://github.com/MidnightCommander/mc/blob/master/po/pl.po), cohort #40. **Ignore all spaces** removes ASCII spaces/tabs from comparison keys, so `a + b` matches `a+b`; it overrides run-collapsing while enabled and combines with case matching. Original text, source coordinates, line boundaries and ending distinctions remain explicit. Retained-source toggles, Swap and Reload preserve ownership and policy. Full suite 3099 passed/3 ignored; six checks zero; native extended options and existing swap workflows each 14/14. This explicit ASCII policy does not claim locale/Unicode whitespace parity. [Evidence](../qualification/diff-ignore-all-spaces-2026-10-31/README.md).

## Optional Windows/Unix line-ending equivalence (2026-10-31)

Independently adapted [Midnight Commander's Strip trailing carriage return option](https://github.com/MidnightCommander/mc/blob/master/po/pl.po), cohort #40. **Ignore CRLF/LF differences** normalizes ending keys independently of text policies and retains raw source snapshots. EOF, bare/internal CR, substantive changes and original coordinates remain explicit; live toggles, Reload and Swap preserve ownership. Full suite 3102 passed/3 ignored; six checks zero; extended native options and existing swap workflows each 14/14. No encoding conversion or source rewriting. [Evidence](../qualification/diff-ignore-crlf-2026-10-31/README.md).

## Positional tab equivalence in text comparisons (2026-10-31)

Independently adapted [Midnight Commander's Ignore tab expansion option](https://github.com/MidnightCommander/mc/blob/master/po/pl.po), cohort #40. **Expand tabs for comparison** uses eight-character stops before case mapping; other spacing, original source text, coordinates and ending policies retain their meaning. Space-ignore modes override the retained preference; toggles, Reload and Swap preserve ownership. Full suite 3105 passed/3 ignored; six checks zero; extended native options and existing swap each 14/14. Unicode scalar columns are explicit, with no display-cell/custom-width parity claim. [Evidence](../qualification/diff-tab-expansion-2026-10-31/README.md).

## Logical versus allocated size in Get Info (2026-10-31)

Independently adapted [Dust's disk usage versus apparent-size distinction](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. **Allocated size** uses captured regular-file Unix block counts and exact bytes alongside logical Size, without Info-time I/O. Unknown, zero, folder and symlink states remain distinct; shared blocks are explicitly not promised as reclaimable space. Full suite 3108 passed/3 ignored; six checks zero; native sparse-file Info and existing symlink Info each 14/14. Recursive allocated totals, treemap modes and Windows support remain open. [Evidence](../qualification/file-allocated-size-2026-10-31/README.md).

## Sort panes by allocated file size (2026-10-31)

Continues the independent [Dust allocation/apparent-size distinction](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. **Sort by allocated size** provides ascending/descending captured regular-file allocation with unknown-last ordering and existing folder/name policies. The Allocated column displays the active metric, accessibility retains both sizes, and Sort by size restores logical values. Cursor, selections and persisted view settings are covered. Full suite 3110 passed/3 ignored; six checks zero; native allocation/logical sorting and existing sparse Info each 14/14. Recursive allocation totals and exclusive/reclaimable accounting remain open. [Evidence](../qualification/allocated-size-sort-2026-10-31/README.md).

## Versioned listing JSON with allocation and selection context (2026-11-01)

Independently adapted [Dust's JSON export](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. **Copy listing as JSON** retains visible/visible-selected ordering with hidden-selection counts, exact paths, regular-file logical/allocated sizes, listed UTC time and applied view/filter context. A bounded 1-MiB writer and lossless path validation fail without staging partial clipboard data. A native-discovered pane expansion was fixed with an owned listing viewport and two UI regressions. Full suite 3116 passed/3 ignored with restored external checksum oracles; six checks zero; native two-report export and prior allocation sort each 14/14. Read-only listing snapshots do not prove current filesystem state or reclaimable storage. [Evidence](../qualification/listing-json-report-2026-11-01/README.md).

## Select the ten largest allocated files (2026-11-01)

Continues the independent [Dust biggest-file/allocation workflow](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. **Select 10 largest allocated files** adds top visible known regular-file allocations with stable path ties, preserving existing/hidden picks. Shared bounded top-N logic retains legacy logical selection; palette availability uses cached observed allocation rather than counting unknowns as zero. Full suite 3119 passed/3 ignored; six checks zero; native allocated-versus-logical selection and previous listing JSON each 14/14. Selection does not estimate exclusive/reclaimable space or collapse hard links. [Evidence](../qualification/allocated-file-selection-2026-11-01/README.md).

## Filesystem identity and hard-link count in Get Info (2026-11-01)

Independently adapted [Midnight Commander's information panel](https://github.com/MidnightCommander/mc/blob/master/src/filemanager/info.c), cohort #40. Captured device/inode and regular-file link counts appear in Get Info and additive listing JSON fields. Full-width values, missing data, link-own identity and omitted directory/link alias counts remain explicit; formatting adds no object I/O. Long values wrap and Info body scrolls beneath a fixed heading/Close. Full suite 3122 passed/3 ignored; six checks zero; native hard-link Info, listing JSON and symlink Info each 14/14. These are listing-time observations, not permanent identities or reclaimable ownership. [Evidence](../qualification/file-identity-info-2026-11-01/README.md).

## Numeric owner and group filters in Find (2026-11-01)

Independently adapted [fd numeric owner/group filtering and exclusions](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. `uid:N` and `gid:N`, optionally prefixed with `!`, combine through AND and survive expression/saved-query round trips. Live lexical Unix ownership and optional cached fields remain distinct from unknown ZIP/legacy metadata; TAR elements use their own header values. Full suite 3125 passed/3 ignored; six checks zero; native four-query ownership workflow and neighboring broken-link search each passed 16/16. Account-name lookup and Windows SID ownership remain open. [Evidence and limits](../qualification/owner-id-search-2026-11-01/README.md).

## Numeric ownership in Get Info and listing JSON (2026-11-01)

Continues the independent [Midnight Commander information panel](https://github.com/MidnightCommander/mc/blob/master/src/filemanager/info.c), cohort #40. **Owner UID** and **Group GID** expose captured lexical Unix IDs for files/folders/links; link values are explicitly marked Link object. Both directory enumeration and one-object observation retain lexical facts without Info-time stat/account lookup. Additive nullable listing JSON fields preserve exact integers and independent unknowns. Full suite 3128 passed/3 ignored; six checks zero; native file Info, symlink Info and listing JSON each 14/14 with raw ownership references. Account-name resolution, Windows SIDs and effective access remain open. [Evidence and limits](../qualification/file-owner-info-2026-11-01/README.md).

## Sort and display access/creation times (2026-11-01)

Independently adapts [eza alternate sort/time fields](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. **Sort by accessed** and **Sort by created** toggle retained timestamp order, date-column display and persisted view settings while preserving cursor/selection. New dates are lexical listing observations, unknown-last in both directions with existing folder/name policies. Get Info and additive UTC JSON fields expose both dates; accessibility preserves Modified and adds the active metric. Full suite 3133 passed/3 ignored; six checks zero; native six-order sort, listing JSON and symlink Info each 14/14 with raw timestamp references. Creation-time support and access-time freshness depend on the filesystem. [Evidence and limits](../qualification/alternate-date-sort-2026-11-01/README.md).

## Structured Find result JSON report (2026-11-01)

Independently adapts [ripgrep structured search output and summaries](https://github.com/BurntSushi/ripgrep/blob/master/crates/core/flags/defs.rs), cohort #76. **Report (JSON)** exports visible or visible-selected results with retained applied query/source, completion/partial counters, pivot order and distinct native/archive addresses. Empty reports, differing drafts, lossless non-UTF-8 paths, explanations and bounded failure before clipboard staging are explicit. Full suite 3137 passed/3 ignored with four test threads; six checks zero; new five-report and legacy twelve-copy native workflows each 16/16. The picker reserves space for eight choices, and a host-load-dependent preview test now queues deterministically before rendering. APFS refuses non-UTF-8 filenames here, so the native fixture verifies a newline filename and records that limitation; byte-path encoding is covered without filesystem I/O. This Commander document is not rg's JSON Lines schema or an operation plan. [Evidence and limits](../qualification/search-json-report-2026-11-01/README.md).

## Search allocated disk space separately from logical size (2026-11-02)

Independently adapts [Dust minimum-size filtering and apparent/disk-size distinction](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. `allocated:` reuses exact/range/strict size grammar for observed regular-file allocation; logical `size:` remains separate. Live/native and optional indexed facts agree, known zero is distinct from unknown, cached values survive source removal, and contradictory link/directory records do not qualify. Missing regular-file allocation stays skipped, including archive payloads; known non-regular kinds are simply excluded. Explanations and search JSON expose observed allocation without extra I/O. [Evidence and limits](../qualification/allocated-query-2026-11-02/README.md).

Validation for allocated search: six checks passed, 3140 tests passed / 3 ignored; native allocation Find and neighboring JSON report each 16/16. Raw block-count comparisons and final source/binary hashes are recorded in the qualification bundle. Windows allocation and archive-member physical allocation remain unestablished.

## Search immediate children of a specific parent (2026-11-02)

Independently adapts the direct-child workflow of [fd search paths and exact depth](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Syntax additionally follows [FSearch](https://github.com/cboxdoerfer/fsearch/wiki/Search-syntax), which is outside the 99-project cohort and is not counted as a cohort source. `parent:"/absolute/folder"` adds a literal immediate-parent constraint shared by live/indexed Find, saved queries, explanations and report context. Native lexical paths and existing archive virtual parents remain distinct from recursive path text matching; regex/case settings do not reinterpret the constraint. Absolute paths without parent traversal are validated after parsing and deserialization. [Evidence and limits](../qualification/parent-query-2026-11-02/README.md).

Parent search validation: six checks passed; 3142 tests passed / 3 ignored; source-bound native parent and neighboring allocation workflows each 16/16. Independent fd 10.4.2 direct-child reference agrees. Native timeout attempts are retained; final accepted capture used a raised window.

## Sort and display captured inode numbers (2026-11-02)

Independently adapts [eza inode sorting and display](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. **Sort by inode** toggles retained inode/device order, keeps unknowns last and preserves existing folder/name policies, cursor and selections. The active column, icon label and accessibility expose lexical identity while retaining logical sizes and Modified metadata. Hard-link aliases retain name ties; links use their own observation. [Evidence and limits](../qualification/inode-sort-2026-11-02/README.md).

The inode native workflow also exposed and fixed complete command-label ranking: Sort by name now precedes the longer Sort by name without extension even when the latter has recent usage. Fuzzy/keyword ordering and empty-query MRU behavior remain covered; the real native workflow retains both name-sort steps.

Inode sort validation: six checks passed; 3145 tests passed / 3 ignored; native six-order sort and neighboring hard-link Info each 14/14 with zero external effects. Independent raw device/inode references and final source/binary hashes passed verification. A screenshot-timeout attempt is retained; final acceptance uses the later captured run.

## Metadata/status change time in lists, Info and JSON (2026-11-02)

Independently adapts [eza changed-time sorting and display](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. **Sort by metadata changed** adds a separate retained Unix ctime metric with signed/subsecond conversion, unknown-last ordering, existing folder/name policies and cursor/selection preservation. The active date column, accessibility, Get Info and additive listing JSON expose the observation; lexical links keep their own time. Created and Modified remain independent. [Evidence and limits](../qualification/metadata-change-time-2026-11-02/README.md).

Metadata change time validation: six checks passed; 3147 tests passed / 3 ignored; native six-order Changed/Modified workflow and neighboring hard-link Info each 14/14. Independent raw signed-second/nanosecond facts, JSON precision, preserved cursor/hidden selection and source/binary hashes passed verification.

## Select the cursor's visible hard-link group (2026-11-02)

Independently adapts [rmlint hard-link group handling](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86, into **Select hard-link aliases of cursor**. Complete retained regular-file device/inode identities select the cursor and its visible aliases; another visible path is required. Copies, symbolic links, directories and incomplete/unknown identities are excluded. Existing/hidden picks and the cursor are preserved; palette availability uses the same observed scope in its existing scan. This is selection over a snapshot, not global alias discovery or a deletion plan. [Evidence and limits](../qualification/hardlink-selection-2026-11-02/README.md).

Hard-link selection validation: six checks passed; 3150 tests passed / 3 ignored; native group selection/JSON and neighboring six-order inode sorting each 14/14. Independent raw identity/link facts, hidden selection, cursor and source/binary hashes passed verification.

## Recursive hard-link identity Find (2026-11-02)

Independently adapts [rmlint hard-link identity and same-file search workflow](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86. **Find hard-link aliases of cursor** captures the originating root and complete regular-file identity in a typed modal request, then runs `device:D inode:N type:regular archives:off`. New exact numeric component filters and `!` exclusions work through live/cached providers, all text modes, saved queries and explanations. Existing index fields are reused independently without fabricating operation identities; missing/archived components stay skipped, and cached facts survive source removal. [Evidence and limits](../qualification/identity-find-2026-11-02/README.md).

Recursive identity Find validation: six checks passed; 3153 tests passed / 3 ignored; native command/editor workflow 16/16 and neighboring visible-group selection 14/14. Raw filesystem identity references, independent find -samefile output, archive-off/on skip counts, root binding and final source/binary hashes passed verification.

## Duplicate groups constrained by extension (2026-11-02)

Independently adapts [rmlint match-extension and optional ASCII case folding](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86. Duplicate Finder Name filters now stages exact or ASCII-insensitive final-extension grouping and applies/reset it through the existing rescan owner. Content verification remains prerequisite; each partition must satisfy minimum-copy and cross-root constraints. Cancellation drops unpublished output, existing hard-link representative ownership remains intact, and JSON context records applied policy independently from drafts. [Evidence and limits](../qualification/duplicate-extensions-2026-11-02/README.md).

Extension-group validation: six checks passed; 3156 tests passed / 3 ignored; native exact/ASCII-insensitive/draft/reset workflow and neighboring name-filter workflow each 14/14, zero external effects. Independent preserved-byte SHA-256 membership, keep plans, proposed counts/bytes, applied JSON and source/binary/tool hash checks passed.

## Duplicate groups constrained by name components (2026-11-02)

Independently adapts [rmlint basename, stem and parent-name matching](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86. Duplicate Finder stages/applies full-name, final-extension-free name or immediate-parent-name grouping, with independent ASCII case folding. Content confirmation remains prerequisite; extension constraints combine through AND, minimum copies and cross-root membership are reapplied, and existing hard-link representative ownership is preserved. Applied summary/JSON remain distinct from drafts. [Evidence and limits](../qualification/duplicate-name-components-2026-11-02/README.md).

Name-component validation: six checks passed; 3161 tests passed / 3 ignored; native 16-state full-name/stem/parent-name workflow and neighboring extension/name-glob workflows each 14/14, with zero external effects. Independent fixture SHA-256, ordered grouping keys, draft/applied JSON, keep indices and proposed counts/bytes matched; final source/binary/tool hashes passed.

## Duplicate keeper ranking by name and path depth (2026-11-02)

Independently adapts [rmlint original-detection ranking](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86: six Keep choices select first/last literal name, shortest/longest native-byte basename, or shallowest/deepest full lexical path. Missing observations stay last and ties retain scan order. Existing apply/skip/sort owners retain exact member indices, and JSON binds the applied policy. No new file reads or removal path added. [Evidence and limits](../qualification/duplicate-keeper-ranking-2026-11-02/README.md).

Keeper ranking validation: six checks passed; 3164 tests passed / 3 ignored; native six-rule ranking/skip/reapply/sort and neighboring chronological/CSV workflows each 14/14. Independent preserved-byte SHA-256 and native name/depth key checks proved six distinct winners and exact report actions/counts/bytes. Final source/binary/tool hashes passed; ranking effects zero, legacy intercepted CSV writes three.

## Duplicate groups constrained by root-relative path (2026-11-02)

Independently adapts [rmlint relative-path matching](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86. Same relative path compares the complete lexical key under the most specific captured root, with optional ASCII folding independent of root ownership. Captured listing origins are now explicit without triggering enumeration; unknown/out-of-root ownership records scan issues. Content, physical-identity, threshold, extension, cross-root, draft/apply/reset and report owners remain shared. [Evidence and limits](../qualification/duplicate-relative-paths-2026-11-02/README.md).

Relative-path validation: six checks passed; 3167 tests passed / 3 ignored; native thirteen-state relative/case/scope/reset workflow and neighboring original-root workflow each 14/14. Independent fixture SHA-256, lexical relative keys, ordered groups, keeper plans and report counts/bytes passed, including an empty single-root state under the existing no-empty-export contract. Final source/binary/tool hashes passed. A real viewport regression also removed the implicit result minimum and replaced the fixed footer reserve with measured style metrics; the expanded two-root UI retains a verified 55.5-pixel result viewport within the window.

## Find files by observed hard-link count (2026-11-02)

Independently adapts [rmlint hard-link inventory/count handling](https://github.com/sahib/rmlint/blob/master/docs/rmlint.1.rst), cohort #86, as Commander-specific `links:` exact/range/strict numeric predicates. Lexical regular-file observations, optional cached counts, missing/archived metadata, full-width integers, explanations and additive Find JSON share existing owners. Count includes aliases outside the root and does not fabricate operation identity or promise reclaimable space. [Evidence and limits](../qualification/hard-link-count-search-2026-11-02/README.md).

Hard-link count Find validation: six checks passed; 3169 tests passed / 3 ignored; native four-query/report workflow and neighboring identity Find each 16/16. Independent raw nlink/device/inode facts and POSIX find -links output matched query membership, explanation counts and copied JSON. Old/missing/contradictory cache observations, zero/MAX, external-root aliases and stale/deleted cached paths passed regressions. Final source/binary/tool hashes passed; exactly one intercepted report-copy effect, all other effects zero.

## Recursive file allocation (2026-11-02)

Independently adapts [dust apparent-size versus allocated-block accounting](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. Disk usage switches between logical lengths and regular-file data blocks with a fresh captured-root scan, metric-specific bounded map ranking, shared tree/filter/share ownership and truthful JSON/CSV metadata. Sparse files, unique hard links, zero/unknown/overflow and cancellation have regressions. Directory/link storage and exclusive/reclaimable extents are outside this measurement. [Implementation checks](../qualification/disk-file-allocation-2026-11-02/README.md). [Completed native metric-switch qualification](../qualification/disk-file-allocation-ui-2026-11-02/README.md).

File allocation native validation: six checks passed; 3173 tests passed / 3 ignored; metric-switch and legacy export scenarios each 14/14. Independent raw block/device/inode references matched root/subtree/map/CSV totals, sparse-file behavior and one-time hard-link accounting. Original-root rescans after panel navigation and final source/binary hashes passed.

## File-count map and tree (2026-11-02)

Independently adapts [dust file count with duplicate inodes (-f -s)](https://github.com/bootandy/dust/blob/master/README.md), cohort #82. File count ranks directories by observed non-directory entries, counting zero-length files, symbolic links and separate hard-link names. Captured-root scans rebuild the bounded map by count; shares/filter/collapse stay on the shared snapshot. Exact entry units and nullable byte/count report fields keep quantities distinct. [Evidence and limits](../qualification/disk-file-count-2026-11-02/README.md).

File count validation: six checks passed; 3175 tests passed / 3 ignored. Count, allocated-block and legacy logical export native scenarios each passed 14/14. Independent raw path/inode/block references matched full totals, compressed subtree/map weights, CSV quantities and selected-metric ranking; captured-root rescans after panel navigation and final source/binary hashes passed.

## Hard-link count listing and ordering (2026-11-02)

Independently adapts [eza hard-link count presentation (-H/--links)](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79, with Commander-specific numeric sorting. Sort by hard-link count exposes retained regular-file counts in list/icon views; unknown values remain last in either direction, preserving folder grouping and name ties. Counts include aliases outside the folder and imply no reclaimable-space guarantee. Existing tab/config/selection/cursor/JSON owners are reused with no metadata reads during sorting. [Evidence and limits](../qualification/hard-link-sort-2026-11-02/README.md).

Hard-link sort validation: six checks passed; 3177 tests passed / 3 ignored; new six-state count/name and neighboring inode workflows each 14/14. Raw nlink/type/inode observations, external-root alias count, exact order, hidden selection/cursor preservation, JSON context and final source/binary/tool hashes passed. Zero native effects.

## Find by access-time boundaries (2026-11-02)

Independent Commander extension combining [fd absolute-time filters](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77, and [eza accessed-time metadata](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. accessed-after:/accessed-before: reuse strict UTC/RFC 3339 comparisons in every match mode and saved queries. New caches preserve exact atime; legacy seconds remain available, invalid exact records stay unknown, and original observations survive source removal. Archive member predicates never borrow container atime used by the existing grouping pivot. Missing metadata remains explicitly skipped. [Evidence and limits](../qualification/accessed-date-search-2026-11-02/README.md).

Access-time filter validation: six checks passed; 3180 tests passed / 3 ignored; native accessed-time and neighboring allocation workflows each 16/16. Independent raw atime seconds/nanos, fractional/pre-epoch query membership, mtime distinction, archive unknown counts, explanations and source/binary/tool hashes passed. Cache precision/legacy/invalid-record, unknown timestamp and stale/deleted source regressions passed. Zero native effects.

## Find by creation-time boundaries (2026-11-02)

Independent Commander extension of [eza creation-time metadata](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. created-after:/created-before: compare captured birth time with strict UTC/RFC 3339 boundaries in all name modes and saved definitions. New exact optional cache records preserve creation observations; absent/unsupported/corrupt metadata stays unknown without mtime/ctime fallback. Nullable native created_utc is included in Find JSON; archived members do not borrow container dates. [Evidence and limits](../qualification/created-date-search-2026-11-02/README.md).

Creation-time filter validation: six checks passed; 3184 tests passed / 3 ignored; native creation and access-time workflows each 16/16. Independent raw APFS birth seconds/nanos, exact UTC strict boundaries, interval/empty/after membership, differing mtimes, archive unknowns, explanations and source/binary/tool hashes passed. Saved-query, precise cache/legacy/invalid/unsupported/stale observations and nullable native creation JSON regressions passed. Zero native effects.

## Find by metadata/status-change boundaries (2026-11-02)

Independent Commander extension of [eza changed-time metadata](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. changed-after:/changed-before: compare captured Unix ctime using strict UTC/RFC 3339 boundaries in every name mode and saved definition. New optional exact cache records and nullable native changed_utc preserve independent status observations without mtime/birth/container substitution. Old/unsupported/corrupt metadata stays unknown and skipped. [Evidence and limits](../qualification/changed-date-search-2026-11-02/README.md).

Status-change filter validation: six checks passed; 3188 tests passed / 3 ignored; native status/creation workflows each 16/16. Independent raw ctime/birth references, UTC boundaries, strict membership and empty results, chmod mode transition with unchanged mtime/size, archived unknowns, explanations and source/binary/tool hashes passed. Query/cache/JSON and stale/unsupported metadata regressions passed. Zero recorded native effects.

## Find creation/status date groups with stable highlight (2026-11-02)

Independent Commander extension of [eza independent creation/status fields](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. Group by adds Created and Metadata changed through existing age buckets, retaining unknown/archive rows under Unknown without timestamp substitution. Wrapped controls preserve room for progress text. Pivot changes now retain the highlighted complete location instead of its old row number; hidden selection, original rank and JSON display order stay shared. No scan rerun or metadata reads added. [Evidence and limits](../qualification/find-time-groups-2026-11-02/README.md).

Date-group validation: six checks passed; 3190 tests passed / 3 ignored; grouping and neighboring status-query native workflows each 16/16. Three actual report copies matched raw metadata/ZIP/container references, pivot buckets/rank and rendered order. Constant query generation and a highlight moving index 1→2→1 while retaining file identity passed independent verification; final source/binary/tool hashes passed.

### Find result sorting — 2026-11-02

Independent Commander adaptation of [eza metadata sorting and reverse](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. Find adds ten retained-result sort keys and descending order; date groups remain primary, unknown metadata stays last, ties preserve worker rank, and highlight follows complete location identity. Rendering, copying and JSON context share the view order; matching, generation and selected/hidden location owners remain intact. Native allocation/link/creation/status observations are not borrowed from archive containers. [Evidence and limits](../qualification/find-result-sort-2026-11-02/README.md).

Find-sort validation: six checks passed; 3194 tests passed / 3 ignored; sorting and neighboring date grouping each native 16/16. Six copied sort reports matched independent raw filesystem/ZIP facts, stable ties/unknown-last ordering, unchanged generation, retained highlight identity and final rendered original rank. The six-row fixture also exposed gradual width growth with larger size labels; result rows now reserve enough space for trailing size/actions. Final source/binary/tool hashes passed.

## Find relative result paths (2026-11-02)

Independent adaptation of [fd relative output](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Copy as adds Relative paths for the visible/selected view, bound to the applied query root and retaining lexical native paths plus archive !/member references. Root becomes a dot; draft roots cannot retarget retained rows. Unbound/outside/traversing/unrepresentable records refuse atomically with visible feedback, without new reads or absolute fallback. Shared view/selection/hidden owners are retained. [Evidence and limits](../qualification/find-relative-paths-2026-11-02/README.md).

Relative-copy validation: six checks passed; 3196 tests passed / 3 ignored; relative-copy and neighboring sort native workflows each 16/16. Actual selected/all copied text independently matched the recorded applied root and rendered paths with unchanged generation; fourteen/six intercepted clipboard writes, other effects zero. Prior parent-folder copying, selection/focus and five-tab checks remain required. UTF-8/control escaping, archive ownership, hidden/selected view, draft binding and refusal/error recovery regressions passed. Final source/binary/tool hashes passed.

## Miller columns with independent listing owners (2026-11-02)

Independent adaptation of [elementary Files Miller view](https://github.com/elementary/files/blob/main/src/View/Miller.vala), cohort #5. Columns is a real per-panel view mode alongside List/Icons, with horizontally scrollable lexical ancestor listings and the current actionable column. Contexts use independent cancellable workers/watchers and avoid recursive size scans; file promotion retains native path identity. Existing current selection/keyboard/context/drop/operation owners remain shared. Leaving the mode, restoring a remembered List folder and tab suspension release contexts. The old listing workload's global path replacement was removed because it cancelled another pane/column for the same path; local generation checks and cancellation bounds remain. [Evidence and limits](../qualification/miller-columns-2026-11-02/README.md).

Miller validation: six checks passed; 3202 tests passed / 3 ignored; six focused regressions cover ownership, async publication/cancellation, configuration, finite layout and end alignment. Four final native workflows (columns, tabs, minimum window, 200% accessibility) each passed 14/14 with zero external effects. Nine path/mode/selection snapshots, bounded context routes and F3 preview content matched independent preserved-source checks; source/binary/tool hashes and frames passed. First-version limits remain fixed widths, up to eight ancestor contexts and pointer promotion; extended Miller parity and the broader cohort scope remain open.

## Per-path Miller column widths (2026-11-02)

Independent continuation of [elementary Files Miller resizing](https://github.com/elementary/files/blob/main/src/View/Miller.vala), cohort #5. Pointer dividers resize individual ancestor/current columns between 180 and 720 UI points; double-click resets that path to automatic width. Each live panel remembers 64 recently changed native paths across navigation, mode switches and tab suspension. Geometry stays outside listing configuration and never schedules a read or clears selection. Widths are not serialized to closed tabs or another process. A recent unrelated click can make egui classify the divider gesture as triple-click; both double/triple reset idempotently. [Evidence and limits](../qualification/miller-column-resize-2026-11-02/README.md).

Width validation: six checks passed; 3205 tests passed / 3 ignored; nine focused Miller regressions passed. Extended native columns and neighboring tabs each 14/14, with zero external effects. Seven additional actual rectangle/model snapshots independently verified 220/360 -> 280/300, List/Columns retention, reset to 360, final 280/300, gaps and unchanged current listing revision/selection. The rapid-click reset classification was observed directly. Both frames and final source/binary/tool hashes passed. Closed-tab/process geometry persistence and remaining Miller/cohort interactions stay open.

## Miller width restoration from the session (2026-11-02)

Continues the independently implemented Miller geometry inspired by elementary Files (cohort #5). Open tabs now serialize their per-path widths through the existing atomic session store. Active and inactive tabs restore separate geometry; a CLI-overridden active path does not inherit the saved tab's widths. Native Unix bytes / Windows units preserve exact path identity, with platform tagging, no filesystem reads and no lossy conversion. Optional malformed rows are ignored, finite widths clamp to 180–720, and snapshots retain at most 64 entries within an approximately 64 KiB encoding budget. Old sessions default to automatic widths. Closed-tab history is still not persisted across processes.

[Checks and qualification limits](../qualification/miller-width-session-2026-11-02/README.md). This milestone validates the real atomic session file plus restoration into fresh workspace objects; it does not claim a new native process-relaunch capture or complete Miller/cohort parity.

Session-width qualification: all six gates passed; 3209 tests passed / 3 ignored. Final implementation hashes match the tested source.

## Miller horizontal keyboard navigation (2026-11-02)

Independent adaptation of [elementary Files Miller key routing](https://github.com/elementary/files/blob/main/src/View/Miller.vala), cohort #5. In Columns, plain Left navigates up with exact native child focus; plain Right enters the highlighted directory only. Right on files or the parent row is inert. Existing modifier, preview, text-input/modal, command-availability and covered-pane routes remain. Enter/Backspace and arrow navigation retain Columns when crossing remembered List folders. [Qualification and limits](../qualification/miller-keyboard-2026-11-02/README.md).

Keyboard validation: 3212 tests passed / 3 ignored, including modifier isolation, retained Columns mode and native pending-focus identity. No new native process capture is claimed.

Miller keyboard native follow-up: the extended native window scene exercises inert Right on a file, Left to the parent with child focus and Right back with cursor/selection restoration; remembered widths remain 280/300 and inactive pane ownership remains unchanged. Fresh Columns and FolderTabs frames were inspected. [Native qualification](../qualification/miller-keyboard-native-2026-11-02/README.md). Physical OS key delivery and independent ancestor focus remain open.

Final native keyboard qualification passed: six gates, 3212 tests / 3 ignored, both native workflows 14/14, all external-effect counters zero, independent artifact/hash verifier passed.

## Closed-tab history across sessions (2026-11-02)

Commander persistence extension of the [Nimble Commander recently closed tabs workflow](https://github.com/mikekazakov/nimble-commander/blob/ee4abffb165019106c7420db5ea8b51623e4baa4/Docs/Help.md). The existing per-pane eight-entry history now survives through the atomic session store: original insertion position, custom title, full-path label, view settings and Miller widths restore lazily without reading closed directories. Latest/index reopen uses existing tab guards and removes the restored record. Tolerant bounded parsing keeps valid records and old sessions; unsupported non-UTF-8 closed roots are omitted consistently with open-tab persistence. Runtime cursor/selection/scroll are not serialized. [Qualification and limits](../qualification/closed-tabs-session-2026-11-02/README.md).

Closed-history validation: six gates passed; 3215 tests passed / 3 ignored; real atomic file round trip, independent pane histories, insertion/presentation/width restoration, tolerant bounded lazy records and non-UTF-8 optional-root omission passed. Final source/validator hash verifier passed.

## Manage the persistent closed-tab history (2026-11-02)

Commander extension of the Ranger/Nimble Commander closed-tab transfer. Closed rows now respect saved custom names and full-path labels; each row offers Forget, with Clear history below. The menu remains enabled at eight open tabs while individual Reopen buttons are disabled. Forget/Clear use the rendered record index, preserve live pane/preview/selection/listing owners, and affect the next saved session without deleting files or opening directories. Existing busy/modal guards apply. [Qualification](../qualification/closed-tabs-management-2026-11-02/README.md).

Closed-history management qualification: six gates passed, 3217 tests / 3 ignored, native tabs and neighboring Columns each 14/14 with zero external effects. Independent snapshots prove history 4 -> 3 -> 0 with unchanged open tabs, right-side history and listing revision; final source/binary/tool hash verifier and inspected frames passed.

## Bounded image EXIF inspector (2026-11-02)

Independent adaptation of [Strata raw metadata](https://github.com/lgse/strata/blob/ede64d928ab6723c71c774948fbf07bc24616cdb/src/sandbox_helper/raw_metadata.rs), using the BSD-2-Clause pure-Rust kamadak-exif parser. Get Info now shows available camera/lens, capture time, exposure/aperture/ISO/focal length, orientation/stored dimensions and GPS fields through a separately owned preview worker. Reads stop at 2 MiB; native object/mtime/change fences, no-follow Unix opens, two-reader admission, cooperative cancellation and a ten-second UI timeout keep it outside the UI/listing owner. Missing/limited/failed data stays explicit; no pixels, maps or metadata writes are involved. [Qualification and remaining scope](../qualification/image-exif-info-2026-11-02/README.md).

EXIF feature validation: six gates passed, 3221 tests / 3 ignored, both native EXIF and neighboring file-identity inspectors 14/14 with zero external-effect calls. Independent Python JPEG/TIFF decoding matches all 14 fixture values and visible GPS; source/binary/tool hashes and inspected frames passed. Additional dependency audit remains failing with the same 13 existing advisories and libbz2 license rejection as a temporary graph without the two newly added EXIF packages; no policy was weakened.

## Image properties without EXIF and four-container qualification (2026-11-02)

Commander continuation of Strata-style rich image properties. Get Info now exposes actual Format and Header dimensions for JPEG/PNG/classic TIFF/WebP, even with missing or broken EXIF, through checked Rust header reads on the existing bounded metadata worker. No raster/pixel decoding is introduced in production. Encoded dimensions and stored EXIF dimensions remain distinct. Seven metadata regressions cover eight real format/EXIF combinations, malformed/CRC/large-header cases, endian/lossy-header variants and EXIF beyond the 2 MiB read boundary. [Qualification and remaining scope](../qualification/image-properties-containers-2026-11-02/README.md).

Image-property validation: six gates passed, 3224 tests / 3 ignored, five native workflows each 14/14 with zero external-effect calls. Independent Python container/IFD parsing matches all displayed fixture properties, proves equivalent EXIF across four containers and checks visible header/GPS rows plus final hashes/frames. Dependency audit remains failing with the unchanged prior diagnostic set; no policy was weakened.

## EXIF attribution, capture offset and exposure modes (2026-11-02)

Commander extension of the rich image metadata transfer: Get Info now includes Artist, Copyright, Software, recorded capture UTC offset/subsecond digits, Exposure program and Metering mode. Camera time is preserved without computer-timezone inference, metadata writes or invented defaults. Existing bounded worker and identity/cancellation contracts stay shared. [Qualification](../qualification/image-exif-details-2026-11-02/README.md).

Details validation: six gates passed; 3225 tests / 3 ignored; native details and PNG-without-EXIF each 14/14 with zero external-effect calls. Independent JPEG/TIFF decoding matches all 23 properties and measured UTC-row visibility; both frames and final source/binary/tool hashes passed. The global dependency audit remains failing with its unchanged prior diagnostic set.

## Image metadata JSON export (2026-11-02)

Commander extension of the Strata-inspired Get Info inspector: Copy metadata JSON exports its retained, matching display snapshot with source identity, exact path encoding and limitation notes, capped at 64 KiB with no file reread. Ready/loading/failure/identity ownership and lossless paths are covered by three new regressions. [Qualification](../qualification/image-metadata-json-2026-11-02/README.md).

Six feature gates passed; 3228 tests / 3 ignored. Native JSON copy and PNG header control each passed 14/14, recording one intentional test clipboard write and zero respectively. Independent JPEG/TIFF decoding matches all 23 actual clipboard properties, raw kernel file facts and final hashes. Global dependency audit retains its prior failing diagnostic set.

## GPX route and elevation preview (2026-11-02)

Independent implementation of [Strata's route-preview capability](https://github.com/lgse/strata/blob/ede64d928ab6723c71c774948fbf07bc24616cdb/src/services/route_preview.rs): GPX now opens as an offline schematic with separate segments, spherical distance estimates, recorded elevation profile and Route/Source switching. Complete source stays within the existing 256 KiB text pipeline; separate Rust parsing adds point/node/concurrency bounds and stale-owner protection. [Qualification and open formats](../qualification/gpx-route-preview-2026-11-02/README.md).

Six feature gates passed; 3232 tests / 3 ignored. Native GPX Source/Route and neighboring image inspector each passed 14/14, with zero external-effect calls. Independently parsed XML agrees on coordinates, elevation, boundaries and 5550.486 m distance. Final frames and source/binary/tool hashes passed. FIT/KML, time statistics, map interaction and 3D remain open; dependency graph/policy are unchanged from the preceding failing audit.

## KML line and gx track preview (2026-11-02)

Continuation of the Strata route-preview transfer: KML LineString/MultiGeometry and gx:Track/MultiTrack now use the shared offline Route/Source viewer, spherical distance and recorded elevation profile. Coordinate ordering, namespaces, sparse gaps and absolute-vs-relative altitude are explicit. Six route regressions pass, including malformed/embedded geometry and existing GPX/cache cases. [Qualification and remaining formats](../qualification/kml-route-preview-2026-11-02/README.md).

Six gates passed; 3234 tests / 3 ignored; native KML and GPX Source/Route workflows each 14/14 with zero external-effect calls. Independent Python parsing of both source formats proves matching points, elevations, boundaries and cumulative 5550.486 m distance; frames and final hashes passed. FIT/KMZ, terrain resolution, interpolation, time statistics and map interaction remain open. Dependency graph/policy are unchanged from the existing failing audit.

## Custom workspace shortcut sequences (2026-11-02)

Independent adaptation of [Kiorg configurable sequences](https://github.com/houqp/kiorg/blob/50aea44c9b4308b98f757574c527b7e0d3a344fe/crates/kiorg/src/config/shortcuts.rs), cohort #1. Keyboard shortcuts now has bounded draft editing, command choices, validated Apply, prefix timeout/cancellation and atomic-session persistence. Exact input ownership, pane/tab/path reset and existing command guards retain one action owner. Invalid saved preferences remain preserved rather than resetting unrelated session state. [Qualification and remaining remapping scope](../qualification/custom-shortcuts-2026-11-02/README.md).

Six feature gates passed; 3240 tests / 3 ignored; native Apply/modal isolation/custom sequence execution and GPX control each 14/14, zero external effects. Actual binding payload, active/inactive visibility and descending natural names match independent expectations. Prefix feedback now disappears immediately when its pending state ends. Frames and final source/binary/tool hashes passed. Graph/policy are unchanged from the existing failing dependency audit.

## Custom navigation bindings and key repeat (2026-11-02)

Continuation of Kiorg's configurable navigation: 16 additional basic cursor/page/selection/pane/entry commands are now offered in the F1 chooser. Single-stroke motions repeat while activation and completed sequences do not. Default/custom navigation share Icons column-aware movement and selection extension. [Qualification and remaining scope](../qualification/custom-navigation-2026-11-02/README.md).

Six gates passed; 3242 tests / 3 ignored. Native List/Icons held-key navigation and previous Apply/sequence/modal-isolation control each passed 14/14 with zero external effects. Known order and exact cursor/selection transitions (List cursor 5, final four-column Icons cursor 9 with seven selected files) independently matched captured state; frames and final hashes passed. Dependency graph/policy are unchanged from the existing failing audit.

## Effective shortcut hints and palette search (2026-11-02)

Continuation of [Kiorg configured-shortcut presentation](https://github.com/houqp/kiorg/blob/50aea44c9b4308b98f757574c527b7e0d3a344fe/crates/kiorg/tests/ui_shortcut_display_test.rs), cohort #1: applied sequences and unshadowed defaults now feed the F1 reference, palette and lower action bar. Shadowed complete/default leader keys disappear; drafts stay isolated. Palette metadata matching now searches effective keys while preserving label/keyword/usage behavior. [Qualification and remaining surfaces](../qualification/effective-shortcut-hints-2026-11-02/README.md).

Six gates passed; 3244 tests / 3 ignored. Native three-stage key/name searching passed 16/16 and previous Apply/sequence/modal-isolation control passed 14/14, zero external effects. Actual rendered hint strings, visible order, keyless Copy action and final F5-to-Reverse display independently matched expectations; frames and final hashes passed. Dependency graph/policy are unchanged from the existing failing audit.

## Shortcut set exchange (2026-11-02)

Commander extension of the Kiorg-inspired shortcut configuration: F1 now copies the active set as versioned bounded Commander JSON and validates pasted JSON into a replaceable draft. Export never reads unfinished edits; import cannot activate bindings before Apply and cannot overwrite draft/live state on validation failure. The reference table now virtualizes visible rows with readable columns. [Qualification and remaining formats](../qualification/shortcut-exchange-2026-11-02/README.md).

Six gates passed; 3247 tests / 3 ignored. Native expansion/export/import/Apply/new-motion and previous sequence/modal-isolation control each passed 14/14. Exactly one intercepted clipboard write, all other effects zero. Actual JSON plus pre/post-Apply owners and cursor independently matched expectations; frames and final source/binary/tool hashes passed. Native input is fixture-seeded, not OS-paste qualification. Dependency graph/policy remain identical to the existing failing audit.

## Saved synchronization comparison profiles (2026-11-02)

Independent adaptation of [Krusader synchronizer profiles](https://docs.kde.org/trunk_kf6/en/krusader/krusader/synchronizer.html): 16 named comparison inputs persist with the atomic session. Explicit replacement, bounded validated storage and fresh comparison on load preserve live guards while discarding old plans, row edits and large-plan acknowledgement. Loading never copies files. [Qualification and limits](../qualification/sync-comparison-profiles-2026-11-02/README.md).

Six gates passed; 3277 tests / 3 ignored. Five new regressions cover collection limits and failure preservation, real asynchronous re-comparison without copying, invalid-load isolation and atomic session reopen. Native UI interaction/layout remains unqualified for this new section. Individual rename/delete, import/export and upstream configuration compatibility remain open. Dependencies and existing audit failures are unchanged.

## Synchronizer profile management (2026-11-02)

Continuation of the Krusader-inspired comparison profiles: rename a selected record using the name field or delete that record without loading a comparison. Collision/invalid-name rejection preserves the book, rename preserves all comparison inputs, and removing the last record leaves a valid empty saved collection. [Evidence and remaining UI qualification](../qualification/sync-profile-management-2026-11-02/README.md).

Six gates passed, full suite 3278 passed / 3 ignored. Source/binary hashes match the checked state. Native UI coverage remains open.

## Swap synchronizer comparison sides (2026-11-02)

Independent adaptation of [Krusader Swap sides](https://docs.kde.org/trunk_kf6/en/krusader/krusader/synchronizer.html): swap root paths/hidden settings, invert mirror policy to retain physical copy intent and recalculate a fresh plan. Old row edits and large-plan approval are discarded; live guards and saved profiles are preserved. [Qualification and native UI gap](../qualification/sync-swap-sides-2026-11-02/README.md).

Final validation: all six gates passed; full suite 3279 passed / 3 ignored. Source and executable identities match the recorded hashes. Native button interaction/layout remains open.

## Synchronizer review masks (2026-11-02)

Adaptation of [Krusader synchronizer mask/selection workflow](https://docs.kde.org/trunk_kf6/en/krusader/krusader/synchronizer.html): optional case-insensitive whole-name glob matching with OR includes and winning exclusions, using Commander comma/semicolon syntax. Existing shown-row actions operate on the matching original targets; filtering alone does not alter Sync. [Evidence and scope](../qualification/sync-review-masks-2026-11-02/README.md).

Final validation: all six gates passed; full suite 3280 passed / 3 ignored. Source/binary hashes were rechecked. Native UI interaction remains unqualified for this new toggle.

## Native synchronizer mask review (2026-11-02)

The existing CI sync_review scene now exercises Name masks with real pointer/key events and Restore/Skip shown; intermediate snapshots prove only notes.md changes while eight hidden planned actions remain intact. Primary and byte-comparison control each passed 15/15; all effects except recorded clipboard exports are zero. Six gates passed, full suite 3280 / 3 ignored, source/binary identities and independent verifier passed; frames inspected. [Evidence and remaining profile/swap/report-mode gaps](../qualification/sync-masks-native-2026-11-02/README.md).

## Synchronizer report match-mode metadata (2026-11-02)

The Krusader-inspired review-mask workflow now exports its matching semantics explicitly: JSON view_match_mode and an appended CSV View match mode column distinguish contains_all_words from whole_name_masks without shifting existing CSV fields. Both scopes/modes and zero-result JSON metadata are covered. [Qualification](../qualification/sync-report-match-mode-2026-11-02/README.md).

Six gates passed, full suite 3281 / 3 ignored. Native review/content controls each 15/15, metadata verified in captured reports and CSV, source/binary hashes match. Control now retries only an unacknowledged initial checkbox click within the unchanged deadline; earlier attempts are retained.

## Native saved synchronizer profile management (2026-11-02)

Extended the existing sync_review CI scenario to save Work, rename it to Renamed and delete it through actual pointer/key input, verifying captured records and unchanged comparison actions. Clarified the name-field hint and wait for collapse animation before capturing the final UI. [Evidence and remaining loading/swap gaps](../qualification/sync-profiles-native-2026-11-02/README.md).

Six gates passed, 3281 tests / 3 ignored. Native profile workflow/content control each 15/15, independent payload/plan/hash verifier passed, final frames inspected. Load/replace/collision/restart and swap UI workflows remain additional coverage.

## Native synchronizer profile load and side swap (2026-11-02)

Actual pointer/key workflow now swaps comparison roots, loads the saved Work profile, proves fresh stamped roots/default row directions, then renames/deletes the profile and exports the rebuilt plan. Earlier Skip edits are discarded as intended and target notes.md remains absent. Six gates passed, 3281 tests / 3 ignored; primary and completed content control each 15/15. [Independent evidence and control-host relaunch caveat](../qualification/sync-profile-load-native-2026-11-02/README.md). Full mirror/non-default native coverage, replacement/collision/restart and unattended macOS control reliability remain open.

## Duplicate keeper preference by captured folder (2026-11-02)

Commander extension inspired by [fclones path-aware keep/removal selection](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md#removing-duplicates): choose one keeper owned by the primary or other captured root, retaining unmatched group choices. Most-specific ownership handles nested roots and original group/member indices remain intact. This is not multi-file keep-path protection; extra copies in the preferred folder may still be proposed for Trash. [Evidence and limits](../qualification/duplicate-folder-keeper-2026-11-02/README.md).

Final validation: all six gates passed, full suite 3282 passed / 3 ignored; exact feature source and executable hashes were rechecked. New native button interaction/layout remains unqualified.

## Native duplicate folder-keeper preference (2026-11-02)

The existing duplicate_cross CI workflow now clicks both folder-preference buttons and exports the resulting plan. Captured JSON proves primary then other-root keepers, unchanged group/member identities and exact two-file / 23-byte proposal; actual members remain present and fixture bytes unchanged. Six gates passed, 3282 / 3 ignored; primary and two-root control each 14/14, independent verifier and final frame inspection passed. [Evidence and remaining nested/unmatched UI coverage](../qualification/duplicate-folder-keeper-native-2026-11-02/README.md).

## Plain duplicate proposal path lists (2026-11-02)

Independent adaptation of [fclones one-path-per-line reports](https://github.com/pkolaczk/fclones/blob/main/fclones/README.md#features): copy chosen keeper paths or proposed Trash candidates in current group order, omitting skipped groups and refusing ambiguous/non-lossless/oversized output before publishing. The existing per-member action owner keeps this consistent with JSON/CSV and the proposed Trash plan. [Qualification and limits](../qualification/duplicate-path-lists-2026-11-02/README.md).

Six gates passed, 3283 tests / 3 ignored. Extended native duplicate workflow and two-root control each 14/14, actual keeper/Trash clipboard lists exactly match JSON actions, independent identity/effect verifier passed and final frames inspected.

## Undo/redo duplicate proposal edits (2026-11-02)

Commander extension inspired by [Kiorg undo/redo](https://github.com/houqp/kiorg#key-features), scoped to the duplicate review plan: restore manual/bulk keeper choices, Keep all and selected rule; no filesystem undo. Bounded scan-local history supports redo branching/no-ops and resets on rescanning. [Evidence and limits](../qualification/duplicate-plan-history-2026-11-02/README.md).

Final validation: all six gates passed, full suite 3285 passed / 3 ignored; source and executable hashes rechecked. New native Undo/Redo interaction/layout remains unqualified.

## Native duplicate proposal Undo/Redo (2026-11-02)

The existing duplicate_cross CI scene now clicks Undo and Redo after root preferences, proving complete proposal equality and history-control availability, then exports the restored plan. Six gates passed, 3285 / 3 ignored; primary and two-root control each 14/14, independent report/path/effect/hash verifier passed, final frames inspected. [Evidence and remaining manual/policy/branching UI coverage](../qualification/duplicate-plan-history-native-2026-11-02/README.md).

## GPX recorded timing statistics (2026-11-02)

Commander extension of the Strata-inspired route preview, using [standard GPX point timestamps](https://www.topografix.com/GPX/1/1/): sum only adjacent positive-time legs within segments, show covered duration/distance/average and missing/invalid/non-increasing counts, retain geometry on bad timing. Explicit RFC3339 zones, fractional seconds and pre-epoch dates are supported; no gap interpolation or moving-speed claim. Six gates passed, 3287 / 3 ignored; native GPX/KML each 14/14 with zero effects, final frames inspected. Independent XML/datetime calculation proves 480 recorded seconds versus 3900 wall-clock seconds and 41.6286472167 km/h. [Evidence and remaining formats/timing scope](../qualification/gpx-timing-2026-11-02/README.md).

## KML gx:Track recorded timing (2026-11-02)

Continuation of the Strata-inspired route preview using [gx:Track parallel time/coordinate arrays](https://developers.google.com/kml/documentation/kmlreference#gxtrack). Preserve original coordinate indices across sparse gaps, accept timing only for equal array counts, and reuse adjacent-positive-leg summaries without interpolating or losing geometry on bad timing. Six gates passed, 3289 / 3 ignored; native KML/GPX each 14/14 with zero effects, final frames inspected. Independent XML/datetime calculation proves partial KML coverage of 300 seconds / 2775.24314778 m / 33.3029177734 km/h and unchanged GPX timing. [Evidence and remaining timing/native formats scope](../qualification/kml-track-timing-2026-11-02/README.md).

## Recorded route elevation gain/loss (2026-11-02)

Commander extension of the Strata-inspired route preview: raw adjacent elevation gain/loss with covered/eligible legs, preserving gaps and unavailable-versus-zero distinction. KML absolute-altitude admission stays shared; no smoothing/terrain correction claimed. Six gates passed, 3291 / 3 ignored; native GPX/KML each 14/14 with zero effects and final frames inspected. Independent XML verification matches every elevation and 90 m ascent / 60 m descent, excluding segment transitions. [Evidence and remaining altitude scope](../qualification/route-elevation-totals-2026-11-02/README.md).

## Route point hover inspector (2026-11-02)

Commander extension of the Strata-inspired route preview: nearest recorded map point within a bounded screen radius is highlighted and exposes point/segment identity, coordinates, recorded distance, elevation and timestamp. Deterministic first-record tie ownership, missing/invalid states and existing segment/date-line projection are retained; no interpolation claimed. Six gates passed, 3293 / 3 ignored; native GPX/KML each 14/14, actual tooltip-closure labels independently match source-verified data, zero effects. Stable frames inspected; transient placement/edge screenshots and coincident/keyboard navigation remain open. [Evidence and QA idle-input correction](../qualification/route-point-inspector-2026-11-02/README.md).

## Copy parsed route JSON (2026-11-02)

Commander extension of the Strata-inspired route preview: export parsed GPX/KML points, segment boundaries, timestamp validity and timing/elevation statistics through the existing deferred clipboard boundary. Versioned model semantics and nullable coverage are explicit; bounded streaming refuses output above 1 MiB without publishing partial text. Six gates passed, 3295 / 3 ignored; native GPX/KML each 14/14, exactly one recorded clipboard call each and zero other effects. Independent verification matches every exported point/statistic to the source-verified route, and final frames were inspected. This derived report does not establish upstream feature parity or route import support. [Evidence and limits](../qualification/route-json-report-2026-11-02/README.md).

## Scoped and inverse table filters (2026-11-02)

Independent adaptation of cohort entry 75, [fzf](https://github.com/junegunn/fzf#search-syntax): numbered-column scope and Exclude matches for CSV/TSV rows, composing with literal/fuzzy case rules and visible-record exports. Missing cells, empty queries, ragged records and source-order ownership are explicit; no fzf syntax/runtime dependency. Six gates passed, 3296 / 3 ignored, including actual egui scope-menu and checkbox pointer tests. Native layout/pasteboard remains open. [Evidence and limits](../qualification/table-scoped-inverse-filter-2026-11-02/README.md).

## Regex and Unicode word table filters (2026-11-02)

Independent adaptation of [ripgrep search](https://github.com/BurntSushi/ripgrep): CSV/TSV Regex and Whole words compose with column scope, exclusion and case policy, using the shared bounded Rust preview compiler. Invalid patterns show persistent errors with no stale records available for copying. Six gates passed, 3297 / 3 ignored; model and egui pointer mode tests passed. Native baseline table workflow passed 14/14 and stable layout was inspected; new native mode/error interactions and OS pasteboard remain open. [Evidence and limits](../qualification/table-regex-word-filter-2026-11-02/README.md).

## Native compound table-filter qualification (2026-11-02)

The table scene now exercises Regex, scoped column selection through the real popup, exclusion, Whole words, invalid-expression exclusion and clear/recovery using actual pointer/text events. All 17 retained states independently match Python CSV/order/matching calculations. Fresh native capture 14/14, two recorded clipboard writes, zero other effects; six gates passed, 3297 / 3 ignored, final frame inspected. This supersedes the native interaction gaps for this compound workflow in the prior two table-filter entries; small-window/error-state screenshots, TSV-specific native flow and OS pasteboard remain open. [Evidence](../qualification/table-filter-native-2026-11-02/README.md).

## Trailing-only comparison whitespace policy (2026-11-02)

Commander extension of the [Double Commander whitespace comparison workflow](https://doublecmd.github.io/doc/en/help.html): Ignore trailing spaces trims ASCII space/tab suffixes in comparison keys, preserving leading/interior content and original display rows. Existing recomparison, swap and explicit reload retain the setting and source ownership. Six gates passed, 3299 / 3 ignored. Native baseline attempted but timed out; no native qualification claimed. [Evidence and limits](../qualification/diff-trailing-spaces-2026-11-02/README.md).

## Single-book zipped FB2 text preview (2026-11-02)

Commander extension of the existing Strata-inspired ebook/archive preview work: .fb2.zip now displays one contained FB2 using the shared encoding/body parser and guarded background snapshot pipeline, without disk extraction. Bounds cover outer ZIP/entry count/declared and actual member bytes; multiple books, unsafe candidates and damaged data fail. Seven gates passed including supply-chain audit, 3302 / 3 ignored; filesystem-provider regression verifies text/cancellation/change ownership. Native compressed-book UI and other wrappers remain open. [Evidence and limits](../qualification/zipped-fb2-preview-2026-11-02/README.md).

## Native zipped FB2 success path (2026-11-02)

Separate compressed-book and raw control scenarios each passed 14/14 on the current executable. The ZIP scene opens a nested uppercase Windows-1251 FB2, finds one phrase and copies the actual text through pointer input. Independent Python ZIP/XML decoding matches all clipboard bytes; both stable frames inspected. Seven gates passed including audit, 3302 / 3 ignored. This closes the prior compressed-book success-path UI gap; error/multi-book layouts, other wrappers and OS pasteboard remain open. [Evidence](../qualification/zipped-fb2-native-2026-11-02/README.md).

## FB2 ZIP collection selection (2026-11-02)

Commander extension of the archive/book preview workflow: background-load a typed bounded collection and select the displayed/copied member with Book, without rereading or extracting files. Native document/cache/model ownership now carries the complete collection; default text providers remain adapted to the same pipeline. Bounds cover 32 books, combined XML/text/names, duplicate and nonregular candidates. Seven regular gates plus final rebuild/menu test passed, 3307 / 3 ignored. Native two-book selection and single-book control each 14/14; actual selected member/clipboard independently match original ZIP/XML, both frames inspected. [Evidence and remaining edge/native scope](../qualification/fb2-book-picker-2026-11-02/README.md).

## Legacy SHA-1 checksum compatibility (2026-11-02)

Independent adaptation of [PeaZip multi-algorithm file checks](https://peazip.github.io/verify-checksum-hash.html), cohort entry 96: streaming SHA-1 in the existing guarded hash lane, explicit comparison/digest/report/GNU list export and BSD/GNU/filename/auto list verification. RustCrypto sha1 0.11.0 is used for legacy compatibility, with bundled MIT notice; SHA-256 stays default and no advisory/license-policy exception is added. Seven gates passed, 3309 / 3 ignored; native existing-plus-SHA1 workflow 14/14 and 16 isolated clipboard writes, actual SHA1 payloads independently checked by Python/shasum. Final frame inspected. Initial fixture/old unsupported-algorithm expectations and an occluded logic-only native attempt are retained; SHA1 list-import UI/OS pasteboard/full upstream parity remain open. [Evidence and scope](../qualification/sha1-checksum-2026-11-02/README.md).

## Captured modification-date rename affixes (2026-11-02)

Inspired by [Double Commander multi-rename date placeholders](https://doublecmd.github.io/doc/en/multirename.html), independently implemented in Rust. The dialog adds a UTC strftime date prefix or suffix to the existing rule. Opening-listing timestamps are keyed by original name, so reordering does not exchange dates. Preview and apply use the same captured snapshot even if a file's modification time subsequently changes. Reopen to refresh the snapshot.

Formats are limited to 64 bytes and formatted output to 512 bytes; unrenderable formats or missing captured timestamps block the whole batch. Existing filename validation and collision detection still apply. Presets save only format/position. This is not Double Commander's mask syntax or a port of its implementation. See [qualification](../qualification/rename-modified-date-2026-11-02/README.md); native pointer-driven date-control qualification remains open.

## Rename and duplicate saved rename presets (2026-11-02)

Independent adaptation of [Double Commander preset management](https://doublecmd.github.io/doc/en/multirename.html): rename a saved label without rewriting its rule, or duplicate a saved rule under a new name. Editor changes remain intact; duplication retains source selection. Canonical name collisions and capacity errors fail atomically. Existing session format is unchanged. [Qualification and interaction limits](../qualification/rename-preset-management-2026-11-02/README.md).

## Saved rename rule at launch (2026-11-02)

Independent adaptation of [Double Commander Preset at launch](https://doublecmd.github.io/doc/en/multirename.html): choose one saved rule for subsequent dialog openings or restore empty-rule startup. The choice survives session save/load and follows rename/update; copies do not inherit it and deletion clears it. Old preset JSON remains readable; ambiguous multi-default JSON is rejected. No automatic filesystem action. [Evidence and native interaction limits](../qualification/rename-launch-preset-2026-11-02/README.md).

## Native saved rename-rule management (2026-11-02)

The native scenario now executes rename, saved-rule duplication, setting/clearing/re-setting launch preference, Escape closure and Cmd+Shift+R reopening. Per-render-frame acknowledgement prevents raw-hook phase loss. Successful 43-phase capture: 16/16 checks, 33 observations independently compared with saved rule semantics, zero native effects. This closes the prior success-path native interaction gaps for these controls; post-animation layout, error paths, date controls and native restart remain open. [Receipts, failed attempts and limits](../qualification/rename-preset-native-2026-11-02/README.md).

## Saved rename preset ordering (2026-11-02)

Independent adaptation of [Double Commander preset sorting](https://doublecmd.github.io/doc/en/multirename.html): ascending/descending canonical Unicode name ordering and manual selected-rule up/down. Saved array order persists; complete rule identity, launch preference and current editor remain intact. [Tests, evidence and native scope](../qualification/rename-preset-order-2026-11-02/README.md).

## Native saved rename-rule ordering (2026-11-02)

Success-path native interaction now covers both sorts and selected-rule up/down after the existing management/reopen workflow: 56 phases, 16/16 checks, 46 observations independently compared to expected order and rule/default ownership, zero native effects. A 300 ms post-input wait fixes the prior fade capture; the visible controls were visually inspected after settling. [Evidence and remaining error/restart/date scope](../qualification/rename-preset-order-native-2026-11-02/README.md).

## Parent-folder rename affixes (2026-11-02)

Independent adaptation of [Double Commander ancestor-name placeholders](https://doublecmd.github.io/doc/en/multirename.html): captured lexical containing-folder/ancestor name as prefix or suffix, configurable level and separator, composing with dates/manual affixes/extensions. The shared planner supplies identical preview/apply semantics and refuses unavailable levels before filesystem mutation. Presets retain policy without retaining a path. [Evidence and explicit mask/native limits](../qualification/rename-parent-folder-2026-11-02/README.md).

## Modified saved rename-rule cancellation (2026-11-02)

Independent adaptation of [Double Commander modified-preset exit behavior](https://doublecmd.github.io/doc/en/multirename.html): Cancel/Escape on an edited selected saved rule requires explicit discard or keeps editing; Rename submission is blocked during the prompt. Native 78-phase workflow verifies both choices and reopening of the unchanged saved rule, 16/16 checks and zero native effects. [Evidence and explicit cancellation/application-quit limits](../qualification/rename-preset-discard-2026-11-02/README.md).

## Native date/folder rename application and Undo (2026-11-02)

Native controls now set UTC year prefix and containing-folder suffix/separator, apply with Enter, then execute the normal Cmd+Z confirmation. Independent Python checks cached proposals against filesystem timestamps/names, actual renamed bytes and complete restoration by Undo. Successful 111-phase workflow: 14/14 final main-pane checks, 101 retained observations, zero clipboard/opener/trash effects and verified owned-fixture mutations. Final restored pane inspected. [Receipts, failed attempts and remaining alternate/native scroll scope](../qualification/rename-metadata-native-2026-11-02/README.md).

## Exact numeric CSV/TSV column sorting (2026-11-02)

Commander extension of the Strata-inspired table preview: Numeric sort compares signed decimal/scientific values without f64 rounding, preserves equal-value source order, keeps invalid/empty/missing cells last, and composes with existing headings, filters and exports. Original cell spelling is retained. This is an independently specified numeric policy, not an upstream parity claim. [Bounds, exact rational oracle, pointer-input tests and native scope](../qualification/table-numeric-sort-2026-11-02/README.md).

## Native exact numeric table sorting (2026-11-02)

Native pointer workflow now compares signed values and adjacent integers beyond f64 precision in both directions, switches between text/numeric/original orders and copies three actual numeric CSV payloads. Python Decimal independently checks 12 new observations plus all 17 existing filter states. Final capture: 133 phases, 14/14 checks and five isolated clipboard calls; active-mode help text corrected and frame inspected. [Evidence and remaining alternate/native OS pasteboard scope](../qualification/table-numeric-native-2026-11-02/README.md).

## Explicit table delimiters (2026-11-02)

Independent adaptation of [Strata delimiter-parameterized parsing](https://github.com/lgse/strata/blob/ac5ab4f22d86798a40d7ad30eb3528d56eecf137/src/services/table.rs): File default / comma / tab / semicolon / pipe selection in CSV/TSV preview, with same-frame bounded reparse and matching escaped export. Choices belong to the pane and captured source; effective separator changes reset column-dependent view state, and new sources restore defaults. No automatic detection or source mutation. [Popup/pointer tests, roundtrip evidence and native scope](../qualification/table-delimiter-2026-11-02/README.md).

## Bounded delimiter detection (2026-11-02)

Commander extension of the Strata-inspired table workflow: Detect delimiter uses quote-aware readers for four separators on the first 32 bounded records, requiring a unique consistent multi-column candidate. Ambiguous or insufficient samples retain the current parse/filter/sort/copy state and expose manual recovery. Success uses the existing active-dialect parse/export path. [Sample limits, allocation/copy ownership tests and native scope](../qualification/table-delimiter-detect-2026-11-02/README.md).

## Native delimiter detection and manual recovery (2026-11-02)

Two native scenarios now qualify a real semicolon file with quoted/multiline data and an ambiguous comma/semicolon file: detect, numeric sort, exact export, manual reset/override, refusal retaining current state and reselecting the same manual delimiter to clear the message. Final captures each 14/14; four isolated clipboard payloads independently checked with Python csv/Decimal. Toast-free final frames inspected; explicit toast-expiry wait is included in these scenes' capture budget. [Receipts, timeout evidence and remaining alternate/native scope](../qualification/table-delimiter-native-2026-11-02/README.md).

## Literal replacement in regex rename (2026-11-02)

Independent adaptation of [Double Commander regex substitution control](https://doublecmd.github.io/doc/en/multirename.html): the regex finder can now insert replacement strings literally, including dollar/capture syntax. The option composes with sequential replacements, persists in presets, and reaches the shared preview/apply planner. Existing rules retain capture expansion. [Qualification and native limits](../qualification/rename-literal-replacement-2026-11-02/README.md).

## Regex literal-replacement pointer input (2026-11-02)

Actual egui pointer input now checks enabling, disabled-click refusal/choice retention and disabling the production control, including same-frame preview and submit-rule changes. Test feature gating corrected. This is headless egui qualification; native window/layout remains open. [Evidence](../qualification/rename-literal-pointer-2026-11-02/README.md).

## Adler-32 file checks and lists (2026-11-02)

Independent adaptation of [PeaZip file checks](https://peazip.github.io/verify-checksum-hash.html), cohort entry 96: streaming Adler-32 joins the existing guarded hash pass, explicit comparison/copy/report/export and checksum-list verification. SHA-256 stays default; tagged/hinted/GNU lists retain separate CRC32 SFV semantics. [Qualification and native scope](../qualification/adler32-checksum-2026-11-02/README.md).

## Native Adler-32 exports and checksum row spacing (2026-11-02)

Native 143-phase checksum workflow selects Adler-32, compares uppercase expected text and copies digest plus relative/absolute lists: 14/14 checks, 19 isolated clipboard calls. Excessive per-newline item spacing in virtual result rows was corrected after live inspection revealed clipped copy buttons. Python zlib independently verifies actual new payloads. [Evidence, retained failures and scope](../qualification/adler32-native-2026-11-02/README.md).

## Base64 checksum copying (2026-11-02)

Independent adaptation of [PeaZip Base64 hash output](https://peazip.github.io/verify-checksum-hash.html): copy the selected completed digest as standard padded Base64 bytes, covering all eight algorithms and retaining the single-result validity guard. Hex comparison and list formats remain explicit. [Qualification and native scope](../qualification/checksum-base64-2026-11-02/README.md).

## Explicit Base64 expected-digest comparison (2026-11-02)

Commander extension of the PeaZip-inspired encoded checksum workflow: explicit Hex/Base64 expected-value selection with strict standard padded Base64 validation and algorithm byte-width checks. Selection rechecks existing text; new calculations default to Hex. Shared completion guards remain intact. [Qualification and native scope](../qualification/checksum-base64-compare-2026-11-02/README.md).

## Copy file details as TSV (2026-11-02)

Independent adaptation of [Double Commander copy file details](https://doublecmd.github.io/doc/en/help.html): command-palette export of captured Name/Size/Modified display cells from visible selection or cursor fallback, without metadata rereads. Quoted TSV preserves control characters and is bounded to 10000 records / 1 MiB. [Evidence and explicit display-column/native limits](../qualification/file-details-tsv-2026-11-02/README.md).

## Native TSV file-details copying (2026-11-02)

Native keyboard workflow invokes the command palette for cursor-only and six-entry selected exports, including a control-character/Unicode filename: 14/14 checks and two isolated clipboard calls. Python csv independently verifies captured cells and exact quoting. [Evidence and native scope](../qualification/file-details-native-2026-11-02/README.md).

## Spreadsheet text policy for TSV file details (2026-11-03)

The Double Commander-inspired TSV command now uses the existing listing CSV apostrophe-prefix policy for formula-like cell starts (=, +, -, @, tab, CR). Prefix bytes participate in the export limit; ordinary cells retain exact quoting behavior. Existing full metadata JSON was verified present and not duplicated. [Regression evidence and importer/native limits](../qualification/file-details-formula-2026-11-03/README.md).

## Native spreadsheet-prefix TSV copying (2026-11-03)

The native palette workflow now exports twelve selected records including names beginning with all six CSV/TSV prefix markers. Python independently applies the prefix rule and verifies the actual quoted clipboard bytes. This qualifies emitted text; spreadsheet importer/OS pasteboard behavior remains separate. [Evidence](../qualification/file-details-formula-native-2026-11-03/README.md).

## Replace prohibited characters in rename targets (2026-11-03)

Independent adaptation of [Double Commander invalid-character replacement](https://doublecmd.github.io/doc/en/multirename.html): optional literal replacement or removal of slash/NUL in final generated names, before validation/collision checks. Default refusal behavior remains; presets and shared preview/apply planner retain the policy. [Bounds, regressions and native/platform limits](../qualification/rename-invalid-characters-2026-11-03/README.md).

## Invalid-character editor pointer/text input (2026-11-03)

Production egui controls now have pointer/text regression coverage for enable, replacement, invalid input, empty stripping and disabled retention with same-frame cached preview/submission checks. Checkbox focus joins the existing form interaction guard. [Evidence and native/full-sheet limits](../qualification/rename-invalid-input-2026-11-03/README.md).

## Native invalid-character rename editor (2026-11-03)

Native 34-phase pointer/text workflow now qualifies replacement, invalid input, empty stripping, disable and re-enable. Actual editor/cache snapshots independently checked against expected target/refusal states; source bytes preserved, zero native-port effects. [Evidence and explicit preview/apply/layout limits](../qualification/rename-sanitize-native-2026-11-03/README.md).

## Ordered name lists for batch rename (2026-11-03)

Independent adaptation of [Double Commander name lists and modifiers](https://doublecmd.github.io/doc/en/multirename.html): pasted/edited complete-name seeds in batch order, strict whole-batch count validation, original-name initialization, preset retention and shared preview/apply composition. Original source identity and date ownership remain intact; collisions/cycles use the existing planner. [Bounds, filesystem Undo/Redo evidence and remaining native/file-import scope](../qualification/rename-name-list-2026-11-03/README.md).

## Edit proposed rename names as a list (2026-11-03)

Independent adaptation of [Double Commander proposed-name list editing](https://doublecmd.github.io/doc/en/multirename.html): turn the current valid preview into an editable name list, resetting already-applied modifiers so targets do not change twice. A shared lossless/bounded constructor also protects original-name initialization. [Plan-equivalence tests and native/external-editor limits](../qualification/rename-preview-name-list-2026-11-03/README.md).

## Native rename name-list editing (2026-11-03)

Native pointer/text flow now exercises preview promotion without repeated modifiers, multiline count refusal/recovery, original-name initialization and a final manual target. Actual editor/cache observations independently checked against fixed expected states; source bytes preserved and zero native-port effects. [Receipts and native apply/layout limits](../qualification/rename-name-lists-native-2026-11-03/README.md).

## Load rename names from a file (2026-11-03)

Independent adaptation of [Double Commander existing name lists, section 5.1](https://doublecmd.github.io/doc/en/multirename.html): bounded cancellable background UTF-8 file loading, then explicit adoption into the list editor with current batch-count checking. Existing modifiers and preview/apply semantics remain active. [Reader, workload/pointer regression evidence and native capture limits](../qualification/rename-name-list-file-2026-11-03/README.md).

## Native name-list file loading (2026-11-03)

Real native pointer/text actions load a wrong-count file, demonstrate disabled adoption without changing the editor, load a valid UTF-8 BOM/CRLF file and adopt it only with a separate click. Read-only snapshots and byte-exact artifacts are independently checked. [Receipts, source hashes and remaining native scope](../qualification/rename-name-list-file-native-2026-11-03/README.md).

## SHA3-256 and SHA3-512 file checks (2026-11-03)

Independent adaptation of [PeaZip file-checking algorithms](https://peazip.github.io/pea-archiving-utility.html): two SHA-3 digests in the guarded streaming reader, Hex/Base64 comparison and copying, and tagged/hinted/GNU checksum-list import/export. Identical SHA-2/SHA-3 digest widths require explicit selection when no tag or filename hint resolves them. [Python oracle, regression and dependency audit evidence; native limits](../qualification/sha3-checksums-2026-11-03/README.md).

## Native SHA-3 workflow and complete digest display (2026-11-03)

Native qualification found the digest result label still enumerated only the prior algorithms. Result display now follows the shared algorithm list and includes SHA3-256/SHA3-512. Pointer/key/text flow covers Hex/Base64 copying, absolute manifests, comparison, malformed-input refusal and recovery for both algorithms, with clip-aware native scrolling. [Independent Python payload checks and native limits](../qualification/sha3-native-2026-11-03/README.md).

## BLAKE2s-256 and BLAKE2b-512 file checks (2026-11-03)

Independent adaptation of [PeaZip BLAKE2 file checking](https://peazip.github.io/verify-checksum-hash.html): full-width unkeyed BLAKE2s/BLAKE2b streaming digests, shared result display, Hex/Base64 comparison/copy, and tagged/hinted/GNU checksum-list export/verification. Existing transitive blake2 is reused directly. [Independent Python vectors, integration regressions and native limits](../qualification/blake2-checksums-2026-11-03/README.md).

## Native BLAKE2 workflow and digest alignment (2026-11-03)

Native pointer/key/text flow exercises both BLAKE2 algorithms, Hex/Base64 copy/comparison, absolute list export, invalid comparison and recovery. Result columns size to the longest algorithm label so BLAKE2 and existing sums align. [Independent Python payload verification and native limits](../qualification/blake2-native-2026-11-03/README.md).

## Forget an individual recent folder (2026-11-03)

Independent adaptation of [zoxide remove](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/man/man1/zoxide-remove.1): Cmd+P can remove its selected recent-folder path and usage rank, immediately saving the session while retaining switcher/input ownership. Revisiting records it anew; bookmarks and panel history are separate. [Snapshot, exact-path and real-egui regressions; native limits](../qualification/forget-recent-folder-2026-11-03/README.md).

## Native recent-folder removal and saved ranking (2026-11-03)

Actual Cmd+P and pointer/text input remove one selected recent-folder record, keep a peer fuzzy match and preserve pane destinations/source bytes. The isolated session service reload confirms removal from both saved paths and saved ranking metadata, with the peer count/recency retained. [Native observations, independent verifier and restart limits](../qualification/forget-recent-native-2026-11-03/README.md).

## Choose recent-folder order in Cmd+P (2026-11-03)

Exposes the existing chronological alternative alongside Commander's [zoxide-inspired frequency/recency ranking](https://github.com/ajeetdsouza/zoxide/wiki/Algorithm). Frequent + recent / Recent first preserve query, pinned favorites and visit metadata; mode changes reset selection, retain focus and save through the session service. [Pointer/Enter, projection and full-session round-trip evidence; native limits](../qualification/recent-folder-order-2026-11-03/README.md).

## Native recent-folder ordering and removal (2026-11-03)

Actual Cmd+P, pointer and text input prove contrasting frequency/chronological projections, selection reset, saved mode changes and removal after reordering. The peer's visit metadata, query and pane destinations are preserved. [Independent snapshot/session verifier and remaining native scope](../qualification/recent-order-native-2026-11-03/README.md).

## Ordered recent-folder path terms (2026-11-03)

Independent adaptation of [zoxide matching rules](https://github.com/ajeetdsouza/zoxide/wiki/Algorithm): optional ordered contiguous path terms with literal separators and final-folder matching in Cmd+P. The existing fuzzy default, original path identities, frequency/chronological choices and bookmark matching remain. Bounded query refusal, selection/focus ownership and persisted mode are explicit. [Upstream examples, limits, pointer/keyboard and session evidence; native scope](../qualification/recent-path-terms-2026-11-03/README.md).

## Native ordered recent-folder path queries (2026-11-03)

Actual pointer/text flow proves ordered fragments, literal slash constraints, reverse/ancestor-only refusal, fuzzy contrast, saved matching mode and over-limit refusal/recovery. Python independently checks native projections and preserves original paths, peer ranking and pane destinations. [Capture, source hashes and visual error/restart limits](../qualification/recent-path-terms-native-2026-11-03/README.md).

## Scope recent destinations to the opening folder (2026-11-03)

Independent adaptation of [zoxide base-directory filtering](https://github.com/ajeetdsouza/zoxide/blob/main/src/db/stream.rs): per-opening Cmd+P scope uses original path components, preserving global recent history and ranking. Source comparison also tightened the ordered matcher's complete final-keyword boundary. [Composition, exact identities, pointer/keyboard evidence and native limits](../qualification/recent-base-scope-2026-11-03/README.md).

## Native recent-folder base scope and reopen reset (2026-11-03)

Actual Cmd+P, scope pointer toggles, Escape and pane switching verify filtering on both panel roots, close-while-enabled/reopen reset and fresh base capture. Query filtering composes with scope while global history/rank and pane directories remain unchanged. [Independent path-component/state verification and native limits](../qualification/recent-base-native-2026-11-03/README.md).

### ZIP member compression and encryption metadata — 2026-11-03

Independent adaptation of [PeaZip per-item method display](https://sourceforge.net/p/peazip/news/2021/09/peazip-820---free-open-source-file-archiver-utility/), cohort #96. Raw ZIP listing now carries compression method (including unsupported numeric identifiers) and encrypted flag without decoding payloads. Member details exposes both and explains unavailable password extraction. CSV appends both fields, distinguishing false from absent metadata. TAR and synthetic folder metadata remain unavailable. Password extraction/encryption-method identification and native visual acceptance remain open. [Qualification](../qualification/archive-method-metadata-2026-11-03/README.md).

### ZIP metadata native acceptance and CSV regression — 2026-11-03

Native ArchiveMetadata now clicks real Stored/Deflated/encrypted unsupported-method ZIP rows, opens Member details and requires all eight rendered fields visible before recording each state. Final encrypted framebuffer inspected; 14/14 common checks and zero native effects. Fresh ArchiveExport retains its five exact recording clipboard writes with two appended CSV columns blank for TAR; 14/14 common checks. Password extraction and encryption-method identification remain open. [Qualification](../qualification/archive-method-native-2026-11-03/README.md).

### BZip2 inside ZIP/ZIPX — 2026-11-03

Independent adaptation of [PeaZip alternative ZIP compression support](https://peazip.github.io/zip-files-utility.html), cohort #96. Enables BZip2 payload decoding using the existing locked dependency; listing, content search, ZIP CRC checks and extraction share the in-process decoder. Recognizes case-insensitive .zipx containers and classifies them as Archives. Python-authored fixtures exercise mixed codecs, Unicode/empty entries, CRC failure, traversal, ratio guards, live/indexed searches, limits and cancellation. Native ArchiveMetadata additionally clicks a BZip2 member in uppercase .ZIPX. Other ZIPX codecs and password extraction remain open. [Qualification](../qualification/zip-bzip2-2026-11-03/README.md).

### Deflate64 ZIP/ZIPX reading — 2026-11-03

Independent integration of [PeaZip alternative ZIP codecs](https://peazip.github.io/zip-files-utility.html), cohort #96. Enables Deflate64 method 9 for listing, bounded content search, CRC checks and extraction, with MIT attribution for deflate64 0.1.12. Independent fixture encoder and system Info-ZIP oracle cover actual extended length 285, full 65536-byte distance, maximum match, empty entry, CRC error and truncated payload. Five new Rust tests plus native ZIPX method visibility cover this feature. Archive creation, other ZIPX codecs and passwords remain open. [Qualification](../qualification/zip-deflate64-2026-11-03/README.md).

### Bounded LZMA ZIP/ZIPX reading — 2026-11-03

Independent adaptation of [PeaZip LZMA ZIP support](https://peazip.github.io/zip-files-utility.html), cohort #96. Shared archive reader adapts method 14 through existing xz2/liblzma with a 64 MiB decoder budget, cached validated properties, EOS/known-size semantics, exact size and CRC checks. Archive content search, integrity checking and regular/symlink extraction all use it; other codecs retain zip-rs readers. Python/liblzma fixtures include actual no-EOS output, malformed/forged dictionaries, encrypted refusal, CRC/size errors and traversal. Eight new Rust regressions and a sixth native metadata state qualify the bounded integration. Other ZIPX codecs, document-reader routing, creation and passwords remain open. [Qualification](../qualification/zip-lzma-2026-11-03/README.md).

### Bounded XZ ZIP/ZIPX reading — 2026-11-03

Independent adaptation of [PeaZip XZ ZIP support](https://peazip.github.io/zip-files-utility.html), cohort #96. Method 95 uses the existing in-process xz2/liblzma decoder with a 64 MiB memory budget enforced in every block/concatenated stream. Shared adapter verifies ZIP size/CRC in addition to XZ checks. Python/xz-produced fixtures cover CRC32/CRC64/SHA-256/unchecked streams, padding/concatenation, late dictionary memory refusal, corruption, traversal and symlinks. Six new Rust tests exercise reading/search/check/extraction; native metadata scenario adds XZ and actual scrolling to virtualized members. Other ZIPX codecs, document routing, creation and passwords remain open. [Qualification](../qualification/zip-xz-2026-11-03/README.md).

### Zstandard ZIP/ZIPX methods 93 and 20 — 2026-11-03

Independent adaptation of [mholt/archives ZIP Zstandard registration](https://github.com/mholt/archives/blob/main/zip.go), cohort #98. Existing zstd dependency now decodes ZIP method 93 and legacy method 20 through the shared verified adapter, with a 64 MiB back-reference-window limit on every frame. Outer ZIP CRC/size, concatenated/skippable frames, no-checksum frames, window-boundary acceptance, later-frame/window refusals and extraction cleanup are covered by CLI-produced fixtures and six new Rust regressions. Native metadata scenario preserves seven prior states and adds exact labels for both method IDs; virtualized scrolling now uses actual row ranks in both directions. PPMd, document routing, creation and passwords remain open. [Qualification](../qualification/zip-zstd-2026-11-03/README.md).

### Bounded PPMd ZIP/ZIPX reading — 2026-11-03

Independent adaptation of [PeaZip PPMd ZIP support](https://peazip.github.io/zip-files-utility.html), cohort #96. Shared verified reader now supports method 98 PPMd8 Restart/CutOff with a 64 MiB model-arena cap validated before allocation. ppmd-rust 1.5.2 adds no transitive dependencies; CC0 notices preserved. Premature raw EOF is converted to a decoding error so missing end markers cannot masquerade as successful full payloads. Seven new Rust tests cover parameters, exact content, CRC/size/truncation, arena boundary/refusal, paths/symlinks, search and integrity budgets. 7-Zip-produced fixtures and independent 7-Zip decoding qualify interoperability; native scenario adds a tenth metadata state. Freeze mode, document routing, creation and passwords remain open. [Qualification](../qualification/zip-ppmd-2026-11-03/README.md).

### ZIP password extraction — 2026-11-03

Adaptation of [PeaZip encrypted ZIP handling](https://peazip.github.io/encrypt-files.html), cohort #96. ZIP/ZIPX extraction now accepts an ephemeral masked password for ZipCrypto and WinZip AES-128/192/256 AE-1/AE-2. All current codecs retain size/CRC, memory/window/arena, path, symlink and overwrite protections. Custom codecs authenticate the whole AES envelope at EOS, with sticky failures, cancellable tail reads and directory verification before creation. Independent 7-Zip/OpenSSL fixtures and Rust tests cover the crypto matrix and negative cases; a native pointer/keyboard scenario verifies wrong/correct Unicode passwords, field clearing, actual byte extraction and repeat preservation. Both final native scenes passed 14/14 gates, and all seven build/test/audit gates passed (3429 tests, 3 ignored). Exact evidence and remaining scope boundaries are recorded in the [qualification](../qualification/zip-password-2026-11-03/README.md). Password-fed search/integrity checks, archive encryption/creation and a password manager remain separate gaps.

### Password-fed ZIP integrity checks — 2026-11-03

Adaptation of [PeaZip archive tests with supplied passwords](https://peazip.github.io/extract-encrypted-files), cohort #96. Test archive now consumes the ephemeral masked ZIP password and verifies every member through the same ZipCrypto/AES and bounded-codec path as extraction. Empty/Unicode passwords, aggregate limits, cancellation, immutable-source checks and complete-report semantics are preserved; no member files are extracted. The regression matrix includes mixed encryption and a failing later member. Native ArchivePassword adds actual wrong/correct-password checks and visible final report/empty field. All seven gates passed (3434 tests, 3 ignored); both native scenes passed 14/14 checks. [Qualification](../qualification/archive-password-check-2026-11-03/README.md). Password-fed content search, per-member password maps, archive encryption/creation and password management remain open.

### Strict ZIP header checks — 2026-11-03

Continuation of cohort #96 [PeaZip-inspired header-corruption checking](https://peazip.github.io/peazip-password.html), implemented through the existing zip-rs validation API. An opt-in Strict ZIP headers control compares local/central metadata and rejects duplicate-name ambiguity before payload checking, while ordinary compatibility mode remains available. Successful reports identify strict mode; encryption, budgets, cancellation and source-stamp checks remain shared. Three new regressions cover eight inconsistent-header cases, valid encrypted/described entries, duplicates and worker propagation. The native password scenario clicks the real checkbox and verifies a successful strict report. All seven gates passed (3437 tests, 3 ignored); both native scenes passed 14/14 checks. [Qualification](../qualification/zip-header-check-2026-11-03/README.md).

### Archive-check report export — 2026-11-03

Adaptation of cohort #96 [PeaZip archive task-log export](https://peazip.github.io/how-to-open-rar-tar-zip-files.html) through Commander's clipboard port. Last check results can be copied as bounded text or versioned JSON, including UTC recording time, complete/incomplete status, actual/captured strict mode, counts or an error. Failed checks preserve unknown counts; changing the checkbox does not rewrite an earlier result's mode. The formatter receives no password, escapes fields and fails explicitly on non-UTF-8 paths/oversize output. Four new regressions and a native four-copy scenario qualify success/error and text/JSON paths. [Qualification](../qualification/archive-check-report-2026-11-03/README.md). Direct file save, per-member logs and password-fed content search remain open.

Qualification: all seven gates passed, 3441 tests passed / 0 failed / 3 ignored; both final native scenarios passed 14/14 checks, including four recording clipboard exports. Evidence: `docs/qualification/archive-check-report-2026-11-03`.

### Declared ZIP encryption method — 2026-11-03

Continuation of cohort #96 [PeaZip ZIP/ZIPX encryption protocols](https://peazip.github.io/encrypt-files.html). Member details now distinguish ZipCrypto and WinZip AES-128/192/256 AE-1/AE-2, and CSV appends Encryption method. Listing uses only declared central-directory metadata, without passwords or payload decoding; unknown strong-encryption declarations remain explicit. Two new regressions cover the six AES combinations and a strong-flag declaration. All seven gates passed (3443 tests, 0 failed, 3 ignored). Native metadata preserves ten prior states plus AES; metadata/export each passed 14/14 checks. [Qualification](../qualification/zip-encryption-metadata-2026-11-03/README.md). Password-fed content search, encryption creation, other encryption protocols and password management remain open.

### One-search password for ZIP content search — 2026-11-03

Independent integration of cohort #96 [PeaZip encrypted-archive access](https://peazip.github.io/extract-encrypted-files) with Commander's existing full-text predicates. Find now accepts an explicitly enabled masked ZIP password for one generation; debounced/fresh-index automatic launches cannot consume it while being entered. The field is consumed/cleared on launch and never enters query/history/index schemas. Live/indexed archive providers share verified bounded ZipCrypto/AES readers; cancellation also polls within raw ZIP reads and AES tails, and incomplete/error content stays skipped. Two new regressions cover live/indexed repeats and immutable index/no extraction, plus thirty custom-codec AES combinations, empty passwords, ZipCrypto, corruption, budget and cancellation. All seven gates passed (3445 tests, 0 failed, 3 ignored). Native password search and ordinary JSON search each passed 16/16; metadata passed 14/14. [Qualification](../qualification/archive-password-search-2026-11-03/README.md). Per-archive/password maps, encryption creation and password management remain open.

### Declared ZIP member CRC-32 — 2026-11-03

Independent adaptation of cohort #96 [PeaZip stored archive checksums](https://peazip.github.io/duplicates-hash-checksum.html). Member details adds Declared CRC-32 and CSV appends CRC-32 (declared), using eight uppercase hexadecimal digits and preserving zero versus unavailable. Listing does not decode or authenticate payloads. AE-2 suppresses even nonzero CRC placeholders; AE-1 and ZipCrypto retain declarations. Two new regressions cover empty/known/forged CRCs and the AE-2 placeholder, with expanded AES/UI/CSV regressions. All seven gates passed (3447 tests, 0 failed, 3 ignored); metadata/export native scenes each passed 14/14. [Qualification](../qualification/archive-crc-metadata-2026-11-03/README.md). CRC sorting/grouping and archive duplicate discovery remain open.

### ZIP CRC-32 ordering and column — 2026-11-03

Independent adaptation of cohort #96 [PeaZip archive CRC sorting](https://peazip.github.io/duplicates-hash-checksum.html). ZIP/ZIPX adds numeric CRC-32 ordering and a clickable checksum column when the pane has room. Unknown CRCs stay last in either direction; explicit folders-first precedence, natural/lexical ties and exact duplicate-entry identities remain stable. No payload reads or selection changes; equal CRCs do not establish byte identity. One new regression covers numeric boundaries, unknowns, names/ties, reverse and field availability. All seven gates passed (3448 tests, 0 failed, 3 ignored); both native scenarios passed 14/14, including real two-direction header clicks and independently verified order/selection snapshots. [Qualification](../qualification/archive-crc-order-2026-11-03/README.md). Automatic archive duplicate discovery remains open.

### Select repeated archive CRC and size — 2026-11-03

Independent adaptation of cohort #96 [PeaZip archive checksum grouping](https://peazip.github.io/duplicates-hash-checksum.html). A ZIP/ZIPX action adds visible regular entries whose declared CRC-32 and size occur more than once, preserving hidden selections and exact entry identities. Missing CRCs, links, folders and special records do not qualify; metadata candidates are not verified byte duplicates. Grouping runs only on click and performs no payload reads. One new regression covers zero/unknown values, distinct sizes/paths/IDs, visibility and TAR. All seven gates passed (3449 tests, 0 failed, 3 ignored); metadata/export scenes each passed 14/14, with six exact native candidates independently recomputed. [Qualification](../qualification/archive-crc-candidates-2026-11-03/README.md). Payload-verified archive duplicate discovery remains open.

### ZIP content SHA-256 groups — 2026-11-03

Independent extension of cohort #96 [PeaZip checksum grouping](https://peazip.github.io/duplicates-hash-checksum.html): background Find matching contents decodes/checks every ZIP member and groups safe regular files by verified SHA-256 and size. Unicode passwords, cancellation, CRC/AES checks, source stamps and decoded/member limits are shared with integrity checking; identity metadata adds an 8 MiB charged budget. Grouping requires strict consistent headers and refuses duplicate-name ambiguity instead of silently coalescing records. Results expose group digests/paths and atomically map original entry identities before selecting matches, including hidden ones. Five regressions cover actual CRC collisions, empty/unsafe/link/directory cases, encryption/refusals, decoded-name budgets, mutation and identity mapping. All seven gates passed (3454 tests, 0 failed, 3 ignored); new content-group and ordinary password native scenarios each passed 14/14, including expanded visible digests/paths and exact selections. [Qualification](../qualification/archive-content-groups-2026-11-03/README.md). This compares SHA-256 and size, not bytes pairwise; duplicate-name containers, TAR content groups, group report export and archive deletion proposals remain open.

### Export verified archive content groups — 2026-11-03

Continuation of cohort #96 [PeaZip archive analysis export](https://peazip.github.io/how-to-open-rar-tar-zip-files.html). Text/JSON check reports now include retained SHA-256 groups with sizes and exact original ZIP indices/paths. Additive nullable JSON fields distinguish unrequested/incomplete from completed empty scans. Serialization streams borrowed groups through the existing 4 MiB writer; escaped text cannot inject fake lines, and unsupported UTF-8 paths or oversized output refuse without partial clipboard updates. No archive rereads or password arguments. Three new regressions qualify identity/escaping/null-empty semantics and limits. All seven gates passed (3457 tests, 0 failed, 3 ignored); both native scenarios passed 14/14, including two exact group exports independently checked against plaintext SHA-256 and four ordinary success/error report copies. [Qualification](../qualification/archive-content-group-export-2026-11-03/README.md). Direct file save and selected-group-only export remain open.

### TAR content SHA-256 groups and complete stream identities — 2026-11-03

Independent combination of cohort #98 [mholt/archives TAR iteration](https://github.com/mholt/archives/blob/main/tar.go) with existing content-hash grouping. Plain/gzip/bzip2/XZ/Zstandard TAR now exposes the same scan, group selection, digest/path view and text/JSON export as ZIP, with explicit TAR checksum limits. Listing/search follows later zero-separated archives and all gzip members so original indices agree with complete scans. Duplicate TAR names retain separate identities; PAX Unicode paths map exactly, and links/folders remain outside regular-file groups. Python/CLI fixtures and two Rust matrix regressions qualify all five codecs, concatenation, later corruption, limits/cancellation and index alignment. All seven gates passed (3459 tests, 0 failed, 3 ignored); TAR-gzip and ZIP native scenarios each passed 14/14 with independent group/selection/report verification. [Qualification](../qualification/tar-content-groups-2026-11-03/README.md). Pairwise byte comparison, archive deletion proposals and duplicate-name ZIP support remain open.

### Per-group archive content selection — 2026-11-03

Continuation of cohort #96 [PeaZip checksum-group workflow](https://peazip.github.io/duplicates-hash-checksum.html). Each rendered SHA-256 archive group now offers Select group and Deselect group, including hidden members while retaining unrelated selections. Group and global actions share atomic original-index/path/size/type resolution; unavailable or stale identities do not partially mutate selection. No rescans, file reads or extraction. One new regression covers independent groups, duplicate paths, unrelated selections and refused mutations. All seven gates passed (3460 tests, 0 failed, 3 ignored); TAR/ZIP native scenes each passed 14/14, with real group-button transitions independently verified. [Qualification](../qualification/archive-group-selection-2026-11-03/README.md). The existing first-100 group render cap remains; pagination and archive deletion proposals are still open.

### Browse all archive content groups — 2026-11-03

Continuation of cohort #96's archive checksum-group workflow. Replaced the first-100-only display with bounded pages and absolute group ranks, allowing access to every retained SHA-256 group. Previous/next controls disable at boundaries, stale state clamps, and new scans reset the page. Per-group actions retain original identity resolution; full selection/export remain independent of the page. Two regressions cover complete page coverage, extreme inputs and actual egui pointer navigation across 201 groups. All seven gates passed (3462 tests, 0 failed, 3 ignored); existing native TAR/ZIP group workflows each passed 14/14 checks. Large-group navigation is qualified by headless egui tests; native scenes cover selection/export regressions. [Qualification](../qualification/archive-group-pages-2026-11-03/README.md). Member-path pagination and archive deletion proposals remain open.

### Browse every member of an archive content group — 2026-11-03

Continuation of cohort #96's archive checksum-group workflow. Replaced the first-ten-only member preview with independent bounded pages for each absolute SHA-256 group. All retained paths can now be viewed with original entry numbers, while group selection/export include all pages. Runtime page state resets on scans/results; group-scoped widgets prevent interference. One new egui pointer regression qualifies independent 21-member groups and the last-member boundary; existing group-page tests qualify shared arithmetic/navigation. All seven gates passed (3463 tests, 0 failed, 3 ignored); native TAR/ZIP integration scenes each passed 14/14. Member paging itself is qualified headlessly. [Qualification](../qualification/archive-member-pages-2026-11-03/README.md). Archive deletion proposals and direct report file save remain open.

### Export an individual archive content group — 2026-11-03

Continuation of cohort #96's archive analysis-report workflow. Each SHA-256 group now offers bounded text/JSON export of every member across all pages. Original group ranks and exact archive indices/paths are retained; an additive JSON content_group_index and explicit text scope distinguish this from a whole-check report. Scan totals remain full-scan metadata, while normal exports retain their previous scope. Shared borrowed serialization preserves UTF-8/output limits and atomic clipboard refusal. Two new regressions qualify nonzero ranks, complete membership, original full-report behavior and refusal boundaries. All seven gates passed (3465 tests, 0 failed, 3 ignored); TAR/ZIP native scenes each passed 14/14, including independent verification of the two new group-export clicks. [Qualification](../qualification/archive-single-group-export-2026-11-03/README.md). Combined arbitrary multi-group export, direct file save and archive deletion proposals remain open.

### Select archive copies while preserving one representative — 2026-11-03

Independent adaptation of cohort #85 [fclones replica preservation and input ordering](https://github.com/pkolaczk/fclones). New per-group/global selection actions choose every scanned matching member except the lowest original archive entry index. Existing keeper selections are removed; unrelated selections remain, and hidden members/all pages are included. All affected identities resolve before mutation, with atomic stale/overlap refusal. This operates on retained SHA-256 groups without archive I/O or deletion. One regression covers duplicate paths, non-monotonic indices, keeper/unrelated selection and failure boundaries. All seven gates passed (3466 tests, 0 failed, 3 ignored); native TAR/ZIP scenes each passed 14/14, including independent keeper/selection verification after real clicks. [Qualification](../qualification/archive-redundant-selection-2026-11-03/README.md). Configurable keeper policies, pairwise comparison and archive deletion proposals remain open.

### Configurable archive replica count and order — 2026-11-03

Independent continuation of cohort #85 [fclones replica preservation and ordering](https://github.com/pkolaczk/fclones). Archive content groups now accept a keep count and first/last original-entry policy shared by group/global actions. Changing the policy leaves selection untouched until applied; kept entries are deselected and unrelated selections remain. Counts at/above group size keep all members; zero refuses atomically. One new regression covers exact count/order with shuffled indices, duplicate names, extreme counts and selection boundaries. All seven gates passed (3467 tests, 0 failed, 3 ignored); native TAR/ZIP each passed 14/14, including real Last entries selection and independently recomputed keeper indices. Numeric count application is qualified by Rust tests. [Qualification](../qualification/archive-replica-policy-2026-11-03/README.md). Path/time priorities, pairwise comparison and archive deletion proposals remain open.

### Exclude path fragments from recent-folder matching — 2026-11-03

Independent adaptation of cohort #75 [fzf inverse-exact tokens](https://github.com/junegunn/fzf#search-syntax). Ordered path terms in Cmd+P now accepts !text exclusions against the entire remembered path, retaining positive term order/final-folder constraints and existing ranking/scope. A leading literal ! can be escaped; empty exclusions refuse, and all terms share the prior count/byte caps. Initial ! retains Omnibar Shell routing, so recent-folder exclusions follow a positive term. One regression covers Unicode, ordering, ancestor exclusions, literal escapes, limits and unchanged usage stats. All seven gates passed (3468 tests, 0 failed, 3 ignored); extended native recent-folder scenario passed 16/16 with independently verified exclusion/refusal/recovery and zero effects. [Qualification](../qualification/recent-path-exclusions-2026-11-03/README.md). Other fzf operators and full syntax parity remain open.

### Anchor recent-folder terms at path boundaries — 2026-11-03

Independent continuation of cohort #75 [fzf prefix/suffix exact syntax](https://github.com/junegunn/fzf#search-syntax). Ordered recent-folder terms now support ^text and text$, plus !^text / !text$ exclusions. Existing positive order/final-folder rules, caps, ranking and mode routing remain; escaped literal operators and empty-token refusals are explicit. One new regression covers combined/inverse anchors, Unicode, escaping, trailing separators and boundaries. Seven gates passed; final build/full tests/strict Clippy were repeated after refinements (3469 tests, 0 failed, 3 ignored). Extended native recent-folder scenario passed 16/16 with an independent Python anchor oracle over 97 snapshots and zero effects. [Qualification](../qualification/recent-path-anchors-2026-11-03/README.md). OR terms, quoted whitespace and complete fzf syntax parity remain open.

### Literal whitespace in recent-folder path terms — 2026-11-03

Independent adaptation of cohort #75 [fzf escaped-space token handling](https://github.com/junegunn/fzf/blob/master/src/pattern.go). Ordered recent-folder queries now accept `project\ alpha` as one literal path fragment, including in exclusions and anchored terms. The bounded character tokenizer preserves non-whitespace backslashes and existing raw-byte/term caps. Go-mode routing also preserves escaped trailing whitespace. Two regressions cover phrases, Unicode whitespace, trailing spaces/backslashes, operators and routing. All seven gates passed (3471 tests, 0 failed, 3 ignored); the native scenario uses a real space in its fixture path and passed 16/16 with an independent oracle over 117 snapshots and zero effects. [Qualification](../qualification/recent-path-spaces-2026-11-03/README.md). OR alternatives and quoted whitespace remain open.

### XXH3-64 and XXH3-128 file checks — 2026-11-03

Independent integration inspired by cohort #85 [fclones XXH3 dependency](https://github.com/pkolaczk/fclones/blob/main/fclones/Cargo.toml). Both default-settings widths now participate in the shared fresh checksum pass, UI selectors/reports, Hex/Base64 comparison/copying and relative/absolute list export/verification. Canonical byte order and leading zeros are retained. Unhinted 32-digit MD5/XXH3-128 lists refuse ambiguous inference; explicit tags/filenames/manual selection disambiguate. The scoped Boost license exception and full attribution accompany the new dependency. Two regressions use 23 official C reference vectors plus list/refusal boundaries. All seven final gates passed (3473 tests, 0 failed, 3 ignored); native checksum scene passed 14/14 with independently checked new comparisons/exports and 38 recording clipboard writes. [Qualification](../qualification/xxh3-checksums-2026-11-03/README.md). This adds algorithm compatibility, without a measured speedup claim; selective execution and seeded modes remain open.

### Calculate only the selected checksum algorithm — 2026-11-03

Independent adaptation of cohort #85 [fclones hash-function selection](https://github.com/pkolaczk/fclones/blob/main/fclones/src/config.rs). The checksum worker can now execute exactly one of fourteen algorithms; unrequested hashers are not initialized/updated/finalized and their digests remain unavailable. New selected-only/all recalculation actions use the complete captured file set and remember the mode for later requests in the running session. List verification also requests only its own algorithm. Scope-specific result display and successful-file counting work without requiring SHA-256 or BLAKE3. Two regressions cover all fourteen values, absent digests, summaries, cancellation and source replacement. All seven final gates passed (3475 tests, 0 failed, 3 ignored); selected/full native scenes each passed 14/14, with exact scope/path verification and all prior 38 clipboard exports preserved. [Qualification](../qualification/selected-checksums-2026-11-03/README.md). Numeric performance qualification and multiple selected algorithm sets remain open.

### Checked checksum algorithm sets — 2026-11-03

Independent adaptation of cohort #96 [PeaZip selected hash algorithms](https://peazip.github.io/verify-checksum-hash.html). A four-column checkbox grid and Recalculate checked set action calculate arbitrary subsets of the fourteen supported algorithms in the shared Rust read loop. Editing the pending set preserves completed results; empty sets disable execution and are refused before I/O. All/single modes, captured paths, cancellation and source-change guards remain shared. All seven final gates passed (3476 tests, 0 failed, 3 ignored). Mixed-set regression and both native checksum scenes (14/14 each, 38 prior clipboard exports preserved) qualify exact requested/pending/available algorithms and prior exports. [Qualification](../qualification/checksum-sets-2026-11-03/README.md). Persistence and measured performance remain open.

### Remember checksum calculation choices — 2026-11-03

Independent continuation of cohort #96 [PeaZip File Tools algorithm settings](https://peazip.github.io/verify-checksum-hash.html). Calculation mode, selected single algorithm and pending checked set now survive session save/restore. Versioned label-based choices preserve empty sets and locally reset invalid/future data without losing the rest of the session. Results and checksum input paths remain transient. All seven gates passed (3478 tests, 0 failed, 3 ignored); native scenes each passed 14/14 with all 38 prior clipboard exports. Rust regressions cover modes/algorithms/invalid data and real atomic file-store reopen; native qualification exercises the App snapshot/store/load/shared restore helper plus a fresh scoped calculation using an injected in-memory store. [Qualification](../qualification/checksum-preferences-2026-11-03/README.md). OS process-relaunch and broader platform-native verification remain open.

### OR alternatives for recent-folder path terms — 2026-11-03

Independent adaptation of cohort #75 [fzf standalone-bar alternatives](https://github.com/junegunn/fzf#search-syntax). Ordered recent-folder terms now support adjacent OR alternatives with AND between groups, including inverse/anchored/escaped-space choices. The last positive fragment rule, order, ranking, scope and caps remain. A bounded two-frontier evaluator avoids Cartesian expansion; malformed operators refuse and literal pipes can be escaped. All seven gates passed (3480 tests, 0 failed, 3 ignored). Explicit and exhaustive-branch Rust regressions plus the native scene (16/16, 157 independently checked observations, zero effects) qualify matching/history/navigation/effects. [Qualification](../qualification/recent-path-or-2026-11-03/README.md). Quoted phrases and full fzf syntax parity remain open.

### Timestamp priorities for matching archive copies — 2026-11-03

Independent adaptation of cohort #85 [fclones replica priorities](https://github.com/pkolaczk/fclones#removing-files). Archive content-group selection can retain the newest or oldest declared member times with the configured count. Known times precede missing ones; ties use lowest original entry indices. Editing the policy leaves selections unchanged until applied. Shared group/global actions retain atomic identity validation and preserve unrelated selections across hidden members/pages. All seven gates passed (3481 tests, 0 failed, 3 ignored). A Rust matrix covers timestamp/count/refusal boundaries; native dated-TAR clicks and ZIP regression each passed 14/14 with independently recomputed keeper identities and preserved exports. [Qualification](../qualification/archive-replica-time-2026-11-03/README.md). Path priorities, archive deletion and broader native timestamp qualification remain open.

### Depth priorities for matching archive copies — 2026-11-03

Independent adaptation of cohort #85 [fclones nested-path priorities](https://raw.githubusercontent.com/pkolaczk/fclones/main/fclones/src/config.rs). Archive content-group selection can keep shallowest or deepest relative paths with the configured replica count. Folder-component depth is distinct from filename length; ties retain lowest original entry indices. Shared group/global actions preserve atomic identity validation and unrelated selections. All seven gates passed (3482 tests, 0 failed, 3 ignored). Rust coverage includes counts, Unicode/hidden/duplicate paths and stale refusal; native TAR performs pending-policy and per-group/global applications, while ZIP retains its integration regression (14/14 each), with independent selection/depth verification. [Qualification](../qualification/archive-replica-depth-2026-11-03/README.md). Preferred path patterns, archive deletion and broader native qualification remain open.

### Protect archive paths from extra-copy selection — 2026-11-03

Independent adaptation of cohort #85 [fclones keep-path patterns](https://raw.githubusercontent.com/pkolaczk/fclones/main/fclones/src/config.rs). A shared case-sensitive archive-relative glob protects every matching scanned member. The keep count becomes a minimum total; the existing priority fills any remaining keeper slots. Edits preserve completed selection, invalid/oversized globs disable both application scopes, and stale/non-UTF-8 paths refuse atomically. All seven final gates passed (3483 tests, 0 failed, 3 ignored). Rust and native TAR coverage qualify protected counts, separator recursion, invalid/recovery and per-group/global effects; TAR and ZIP integration each passed 14/14, with independent keeper verification and unchanged export effects. [Qualification](../qualification/archive-protected-paths-2026-11-03/README.md). Persistent presets, archive deletion and broader native qualification remain open.

### Protect archive filenames at every depth and choose letter-case matching — 2026-11-03

Independent adaptation of cohort #85 [fclones keep-name and keep-path](https://raw.githubusercontent.com/pkolaczk/fclones/main/fclones/src/config.rs). Archive protection adds Filename only and Match case, reusing the bounded archive glob matcher. Full-path/case-sensitive defaults stay intact; lowercase-based matching supports explicit case-insensitive protection. Pending option edits preserve current selection, and per-group/global application retains protected-count and identity guarantees. Rust and a dedicated 70-phase native TAR scene compare scope/case combinations; the previous 149-phase TAR regression and ZIP integration regression remain separate. All seven final gates passed (3484 tests, 0 failed, 3 ignored); all three native scenes passed 14/14 with independent keeper/source verification and clipboard effects 0/4/2. [Qualification](../qualification/archive-protection-names-2026-11-03/README.md). Persistent presets and broader native qualification remain open.

### Multiple archive protection patterns — 2026-11-03

Independent adaptation of cohort #85 [fclones multiple keep-name/keep-path patterns](https://raw.githubusercontent.com/pkolaczk/fclones/main/fclones/src/config.rs). Archive protection accepts newline-separated globs with OR matching, shared filename/case modes and minimum-total keeper semantics. Up to 16 nonempty patterns, 256 characters each and 8192 total characters; blank lines are ignored, CRLF is accepted and spaces stay literal. Every line must compile before group/global selection; errors identify their line and preserve selection atomically. The Rust matrix and nine extra native checkpoints cover list application, invalid/recovery, clearing and restoration; the earlier mode, TAR and ZIP regressions remain included. All seven gates passed (3485 tests, 0 failed, 3 ignored), all three native scenes passed 14/14, and independent source/keeper/export verification passed. [Qualification](../qualification/archive-protection-lists-2026-11-03/README.md). Persistent presets and broader native qualification remain open.

### Restrict archive copy selection to old groups — 2026-11-03

Independent adaptation of cohort #85 [fclones modified-before group restriction](https://raw.githubusercontent.com/pkolaczk/fclones/main/fclones/src/config.rs). A UTC date or RFC 3339 cutoff applies Select copies only when every group member has a known declared modification time strictly before the boundary. Equal/newer/unknown dates skip the whole group and preserve existing selection; empty disables the limit. Pending edits, invalid-input refusal and atomic source/overlap checks remain shared by group/global actions. Rust coverage includes offsets, fractions, pre-epoch times, mixed/unknown groups, protected patterns and stale skipped identities. All seven gates passed (3487 tests, 0 failed, 3 ignored); all three native scenes passed 14/14, with 12 new cutoff checkpoints independently verified and existing exports preserved. [Qualification](../qualification/archive-group-cutoff-2026-11-03/README.md). Persistent presets and broader native qualification remain open.

### Restrict extra archive copies to candidate patterns — 2026-11-03

Independent adaptation of cohort #85 [fclones name/path action restrictions](https://raw.githubusercontent.com/pkolaczk/fclones/main/fclones/src/config.rs). A bounded newline-separated candidate glob list restricts which scanned members may be selected as extra copies. Nonmatches and protected members are kept and count toward the minimum; protection wins, while date-skipped groups preserve selection. Filename/case options explicitly apply to both lists. Empty candidates preserve earlier behavior; invalid lists disable both action scopes and refuse atomically. Rust covers both scopes, matching/count interactions, stale and non-UTF-8 refusal. All seven gates passed (3488 tests, 0 failed, 3 ignored); all three native scenes passed 14/14 with ten new candidate checkpoints independently verified and existing exports preserved. [Qualification](../qualification/archive-copy-candidates-2026-11-03/README.md). Persistent presets and broader native qualification remain open.
