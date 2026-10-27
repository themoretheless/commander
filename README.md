# Commander

A dual-pane file manager for macOS, built in Rust with [egui](https://github.com/emilk/egui).

Two side-by-side panels, keyboard-first navigation, and native macOS
integration (Quick Look, Finder tags, share sheet, APFS clone copies).

The file-manager logic lives in a UI-independent, unit-tested core; the `app`
module is a thin egui layer over it.

## Features

### Panels and navigation

- **Folder tabs in each pane**: `Cmd+T` opens a tab, `Cmd+Shift+W` closes the
  current tab, and `Cmd+Shift+[` / `]` cycle tabs. Each pane supports eight tabs;
  directory, selection, history and view settings stay independent. Open folders
  and per-tab view settings restore after restart; inactive tabs load lazily.
  `Cmd+Shift+T` restores a closed tab with its runtime state; **Closed** lets you
  choose among the last eight closed tabs in that pane (current run only).
  Middle-click a tab, or use **Close this folder tab** in its context menu, to
  close it directly. Background closes preserve the live listing/preview and
  pane focus; at least one tab remains in each pane.
  **Duplicate this folder tab** copies the pointed tab's folder and view
  settings beside it in the background, including when that tab is inactive.
  The live selection, filter, preview and pane focus remain unchanged; copies
  load lazily and count toward the pane's eight-tab limit.
  **Move this folder tab to other pane** transfers the tab with its runtime
  selection, filter, view and navigation history beside the other pane's active
  tab, in the background. Keep at least one source tab and space for the moved
  tab in the destination. Moving an inactive tab preserves both current folders;
  moving the visible tab opens its neighbor in the source pane.
  A tab's context menu offers **Close other folder tabs** and **Close folder
  tabs to the left/right**. These act relative to that tab, including inactive tabs;
  closed tabs remain recoverable through the same bounded history.
  **Copy this folder path** copies the pointed tab's folder without activating
  it or changing the current selection. Control characters use the same
  single-line escaping as other copied paths.
  A tab's context menu also edits its **Tab title** (up to 64 characters).
  **Use folder name** restores the automatic label. Titles follow the tab through
  navigation, duplication, copying, moving and closed-tab recovery, and are saved
  between sessions. The full folder path remains in the tooltip.
  **Protect from closing** marks a tab with `*` and blocks direct/middle-click
  closing. Close groups and duplicate cleanup keep protected tabs. Navigation
  remains available. Protection is saved and follows duplication/copy/move;
  turn it off in the tab menu before closing the tab. The command palette offers
  **Protect folder tab from closing** and **Unprotect folder tab** for the current
  tab, with availability reflecting its protection state.
  Duplicate cleanup keeps every protected tab. Otherwise it prefers a custom
  title, then the active tab among ties, then the first. If the active copy is
  removed, its surviving peer becomes visible and the removed runtime tab can
  be reopened from Closed history.
  **First folder tab** and **Last folder tab** in the command palette jump to
  either end of the current pane's tab strip and restore that tab's state.
  Each command is unavailable when its destination is already active.
  **Go to folder tab 1–8** selects a numbered tab directly from the palette;
  missing tabs and the current tab are unavailable. Cmd+number keeps its
  existing saved-folder shortcut behavior.
  The palette also offers **Close other folder tabs**, **Close folder tabs to
  the left** and **Close folder tabs to the right**, relative to the current tab.
  Protected tabs survive; groups with no eligible tabs are unavailable in both
  the palette and tab menu. Closed tabs retain their state in Closed history.
  **Open highlighted folder in background tab** is available in the command
  palette and by right-clicking **+**. It inserts beside the current tab without
  switching away; contents load when the new tab is activated.
  **Copy all folder tabs to other pane** (palette or a tab's context menu)
  copies the whole group and its view settings beside the other pane's current
  tab. Both current folders stay open; the group must fit within eight tabs.

- **List and Icons views** per pane, with bounded image thumbnails in the active
  grid, the same selection/drag/context-menu operations, and remembered view
  mode. Choose a mode in the pane header, View menu, or command palette.
- **Keyboard reference** (`F1`) lists catalog shortcuts with unavailable reasons.

- **Dual panels** with an active-panel marker (`Tab` switches sides), a
  breadcrumb path bar, and a folder tree sidebar with a **Favorites rail**.
- **Bookmarks** with quick-jump slots: `Cmd+1`..`9` jump the active panel to a
  pinned folder, `Cmd+Shift+1`..`9` assign one.
- **Per-pane history**: `Cmd+[` / `Cmd+]` walk back and forward through the
  directories you visited (a vim-style jump trail).
- **Go to path** (`Cmd+L`), **recent folders** (`Cmd+P`, last 200 visited
  folders ranked by frecency or chronology), type-ahead jump, and a per-panel
  filter box with quick-filter
  facets (kind, size, and date buckets: Today / Week / Month, plus an
  Older-than-a-month bucket).
- **Command palette** (`Cmd+K`): fuzzy-filter every command, ranked by recency
  and frequency. Availability comes from the same workspace policy as both
  toolbar layouts; unavailable matches stay visible with a reason, and Enter
  selects the first action that can actually run. Direct shortcuts use the
  same typed predicates and explain a refused action instead of disappearing.
- **Streaming search** with cancellable generations, stable result identity,
  one fielded query grammar, Exact/Fuzzy/Regex modes, replayable history,
  optional inspectable content indexes, and bounded ZIP-member search.
- **Project collections** combine multiple roots into non-owning virtual views;
  the compressed disk tree stays cancellable and bounded on large roots.
- **Per-folder view memory**: sort, filters, hidden, and density are
  remembered per directory for the running session (not persisted across
  restarts) and restored when you navigate back.
- **Vim-style chords**, modifier-free: `j` / `k` move the cursor down/up
  (with a leading count, e.g. `5j`, picked up from the type-ahead buffer),
  `g g` jumps to the top, and `s s` reverses the sort.

### File operations

- **Batch rename presets** (`Cmd+Shift+R`): save up to 32 named rules, reload
  them into the live preview, explicitly update or delete saved rules. Presets
  persist with session settings; loading one never applies a rename by itself.
  **Reset rule** clears current edits and restores initial counter controls,
  detaches the selected preset and rebuilds an unchanged-name preview. Saved
  presets and the target-file list are retained.
  Find/replace can target the whole name, its stem, or just the last extension;
  the selected scope is saved with each rule. Extensions exclude their dot.
  **Ignore case** and **First match only** work with both literal and regex
  replacement and are remembered in presets. Literal replacement keeps `$1`
  as text; regex replacement supports capture references.
  Case modes include **First capital** and **Title Words** (Unicode word
  boundaries). Independent **Extension case** options Keep / lower / UPPER
  change only the resulting last extension after find/replace; Keep is the
  default. Prefixes and suffixes are added after stem case conversion.
  These modes are saved in presets too. Numbering accepts negative starts and
  steps for descending sequences across zero; a zero step repeats one number.
  Padding includes the minus sign. Signed numbering is saved in presets.
  **Counter position** places the number before or after the decorated name
  stem, retaining the final extension. After name remains the default for old
  rules; the position is saved in presets. Prefix text can provide a separator
  after a leading counter, for example `03_Photo.jpg`.
  **Counter only** replaces the decorated stem with the number while retaining
  the resulting extension (`Photo.JPG` → `001.JPG`). This mode is saved in
  presets and has no effect while numbering is disabled.
  **Trim spaces** removes Unicode whitespace at both ends of the final name,
  after other transformations. Internal spaces remain intact. This option is
  saved in presets; empty names and collisions still block renaming.
  **Collapse repeats** reduces consecutive identical punctuation, whitespace or
  symbol graphemes to one (`report___final--v22.txt` → `report_final-v22.txt`).
  Letters, digits and different adjacent separators remain intact. It runs on
  the final name before Trim spaces and is saved in presets.
  **Unicode** offers Keep / NFC / NFD / NFKC / NFKD normalization of the final
  complete name. Keep is the default. Compatibility forms also replace characters
  such as ① with 1; resulting invalid names and collisions block renaming.
  The selected form is saved in presets. A filesystem may enforce its own form.
  **Remove prefix / Remove suffix** remove one exact, case-sensitive match from
  the original complete name, before find/replace and extension splitting
  (`backup_photo.jpg.old` → `photo.jpg`). Empty fields do nothing. Both are
  saved in presets and cleared by Reset rule.
  **Limit stem** keeps up to 1–255 complete Unicode graphemes in the final stem,
  retaining the last extension. It runs last, including after decorations and
  normalization; folders use their whole name. The option defaults off, saves
  in presets and resets to an inactive limit of 32. Filesystem byte-length and
  collision checks still apply.

- **Copy / move / delete** on a background thread with a live progress window
  (speed graph, ETA, per-file error list). Copies use native `copyfile` with
  APFS cloning and fall back to a buffered copy; a same-volume move is an
  instant atomic rename.
- **Durable operations**: Fast/Verified/Versioned profiles, typed failures,
  source/destination revalidation, an idempotent operation journal, safe-state
  lockout, and a Recovery Center with Resume, Roll back, Inspect, and repair
  plans. Versioned operations expose Compact/Recent/Archive/Forever retention;
  pruning publishes the retained manifest before deleting expired copies.
- **Adaptive transfers**: resumable buffered and delta checkpoints, fixed or
  measured-threshold FastCDC delta copy, sparse-file preservation, explicit
  clone/rename/delta/buffered telemetry, per-volume concurrency, bandwidth and
  quiet-hour rules, and a generation-aware verified BLAKE3 cache.
- **Transfer queue**: firing a second operation while one runs queues it
  instead of dropping it (F5/F6 stay live during an active transfer just for
  this). The **queue panel** (palette: "Transfer queue") lists every waiting
  job with pause / resume / reorder / cancel; concurrency stays capped at one
  transfer at a time (the progress UI is built around a single active job).
- **Operation history** (palette: "Operation history"): a searchable log of
  completed moves, deletes, and batch renames, each with a jump-back button
  and, while it is still the exact top of the undo stack, a live Undo button.
  Session-lifetime, not persisted across restarts.
- **Safe overwrites**: the confirmation dialog offers Overwrite All, **Keep
  Both**, and Skip. Overwrites stage the new copy and swap it into place, so an
  interrupted copy never destroys the existing file. Copying or moving a path
  into itself is rejected, and a **free-space preflight** warns before a copy
  that will not fit (a clone or same-volume move needs ~0 extra space).
  Visible actions also consume the shared volume capability matrix: Copy needs
  a writable destination, Move needs both panes writable, and local mutations
  are disabled on a read-only active volume with the exact reason shown.
- **Drag and drop** between panels and onto subfolders, routed through the same
  engine as the keyboard. `Cmd+Enter` moves into the highlighted folder and
  `Cmd+Shift+Enter` copies into it; both actions are also available from the
  toolbar overflow menu for single-pointer use.
- **Archive browse / extract**: open `.zip`, `.tar`, `.tar.gz` and `.tgz` as a
  member browser, multi-select entries, and extract into the other panel.
  Switch between All members and Folders, navigate with breadcrumbs/Up, and
  select a listed folder subtree without losing selections in other folders.
  Sort archive rows by name, size, packed size, compression ratio or date;
  reverse the order or put folders first without losing the selection.
  **Natural names** sorts file2 before file10 (including navigation folders);
  turn it off for ordinary name ordering.
  Text search also visits bounded TAR/tar.gz members. Automatic archive
  inspection in search consults trust-gate hooks (J003).
- **View as hex** (palette): inspect binary files in 4 KiB pages with absolute
  offsets, hexadecimal bytes and ASCII. Previous/Next, First/Last, decimal/hex
  Go to offset, Reload and Copy page work without editing the file.
  Switch **HEX / Decimal** to display byte values in base 16 or 10; Copy page
  uses the selected representation. Row offsets remain hexadecimal and byte
  search still takes hexadecimal input. A new viewer defaults to HEX.
  Find next/previous search exact hexadecimal byte sequences throughout the file,
  across page boundaries, with progress and cancellation.
- **Sort: folders by name first (toggle)** (palette): keeps folders first in
  ascending name order while files follow the chosen column and direction.
  Natural name ordering and case settings apply to folders too. Enabling it
  turns on folders-first; turning folders-first off clears this option. The
  option is saved with pane/tab views and is off by default.
- **Sort: case-sensitive names (toggle)** (palette): uses original Unicode names
  for name/stem sorting and name tie-breaks; ASCII uppercase sorts before
  lowercase. Natural numeric order still applies when enabled. This mode keeps
  accents and does not use locale collation. It is off by default and saved in
  each pane/tab's view settings.
- **Sort by name without extension** (palette): compares file names before their
  final extension, so `a.txt` precedes `a-z.txt`. Directory names keep their dots.
  Natural name ordering and folders-first still apply; equal stems use full
  names in ascending order. Run again to reverse direction. The column heading
  identifies this mode, and tab/session view settings retain it.
- **Selection memory** (palette): **Save selection in memory** remembers this
  folder's explicit picks, including filtered-out picks; **Restore saved
  selection** replaces this folder's selection with the saved paths still in
  its complete listing. Picks in other folders stay intact. Each tab has one
  in-memory buffer, bound to its saved folder; return to that folder to restore
  it. Saving an empty selection deliberately replaces the buffer. The buffer
  follows closed-tab recovery and moves, but is not saved across app restarts
  or copied when duplicating a tab.
- **Checksum / verify** (palette): hash the selection with the existing
  content-hash, BLAKE3 and SHA-256 in one fresh pass. Compare a single file
  with an expected SHA-256 or BLAKE3 using an explicit algorithm selector.
  Copy the selected single-file digest, a report or a SHA-256 checksum list.
- **Create symlink / hard link** in the other panel from the selection.
- **CLI launch**: `commander <left> [right]` opens those folders on startup.
- **Run command output**: captures stdout/stderr/exit instead of fire-and-forget.
- **Navigation lock**, **`.` repeat last command**, **named bookmarks in the
  palette**, and **Sync dry-run** preview.
- **Go to home folder** (`Cmd+Shift+H`) and **Go to disk root** (palette) use
  the active tab's normal navigation history. The opposite pane stays put,
  including with navigation lock enabled. `Cmd+H` still toggles hidden files.
- **Open highlighted folder in other pane** (palette or folder-tab context
  menu): browse the cursor folder in the opposite pane's active tab while
  retaining source focus, selection and filters. The destination's Back history
  retains its previous folder; other tabs stay open, including at tab capacity.
- **Gather into a new subfolder** (`Cmd+Shift+N`): move the selection into a
  freshly-named folder in one undoable step (Finder's New Folder with
  Selection). Undo moves the files back and removes the empty folder; redo
  recreates it before gathering again.
- **Batch-rename studio** (`Cmd+Shift+R`): find/replace (plain or **regex**,
  with `$1`-style capture groups), case, prefix/suffix, numbering, with a live
  preview. Resolvable collisions (swaps, rotations, and the case-only
  `Foo` -> `foo` rename) are applied through a safe temp-staged order.
- **Run-command / open-with bar** (palette): run a shell command on the
  selection with `{paths}` / `{names}` / `{dir}` placeholders expanded and
  shell-quoted, with a live preview; save reusable templates.

### Comparing and selecting

- **Duplicate finder**, read-only **text diff** (`Cmd+D`), directory
  **synchronise** sheet (`Cmd+Shift+S`), and a disk-usage **treemap**
  (`Cmd+Shift+M`).
- **Compare mode** tints rows by how they differ from the other panel, with
  relative-size occupancy bars. Same-named folders and file/folder collisions
  have explicit states; directories are never called identical merely because
  their metadata-sized fingerprints happen to match.
- **Cross-pane selection**: select files only in this panel, differing from the
  other, identical to the other, or same-named (palette).
- **Selection algebra**: select-by-mask (`Cmd+G`), and stash/union/intersect/
  subtract/symmetric-difference of selections.
  The command palette can select or deselect visible files with the highlighted
  file's name ignoring its last extension (`report.pdf`, `report.docx`, `report`).
  Matching ignores case; folders and unrelated or filtered-out picks remain.
  **Go to first/last visible file** moves the cursor past folders according to
  the current sort and filter, retaining selections and the repeat-command target.
  Both actions are available in the command palette when files are visible.
  **Invert visible file selection / Invert visible folder selection** flip only
  the chosen kind in the current view. The other kind and filtered-out selections
  remain intact. Both are available in the command palette.
  Masks accept commas or semicolons, including mixed separators
  (`*.jpg;*.png,!*raw*`). Subtraction still wins. Use Exact names for literal
  filenames containing these separators.
  Optional **Match case** distinguishes uppercase/lowercase in masks, including
  extension shorthand and exclusions. It starts off each time the dialog opens;
  Exact names remains case-sensitive independently of this option.
  **Ignore accents** compares masks, names and extensions after canonical
  decomposition and removal of combining marks, expanding common Latin
  ligatures (`café` → `cafe`, `cœur` → `coeur`). It works with Match case and
  exclusions, defaults off for each dialog and never renames files. Exact names
  disables this comparison option.
  **Treat `*.*` as `*`** lets the exact
  `*.*` term match extensionless names too, including in exclusions. Other masks
  retain their usual meaning. This optional compatibility rule starts off with
  each dialog and is disabled for Exact names.
  `Cmd+G` also offers **Exact names (one per line)** for pasted filename lists:
  literal, case-sensitive, additive, and limited to the current filtered view.
  Lists are bounded to 64 KiB / 4096 names; invalid paths block application.
  Enter adds a line; Cmd+Enter or Select applies it.
  **Apply to** limits mask/list changes to files, folders, or both, with the
  live count following that choice. Entries of the other kind keep their marks.
- **Selection groups** (palette): add visible files or visible folders to the
  current selection, or deselect visible files sharing the cursor's extension.
  Existing selections outside the current filter are retained; folder names never
  participate in extension subtraction.
- **Marked-files set** (`M` to toggle), distinct from the interactive
  selection and never cleared by select-all/invert/clear-selection, with its
  own union/intersect/subtract/symmetric-difference against the selection
  (palette).
- **Drop-stack shelf**: gather files across folders (`Cmd+Shift+A`) and drain
  them into one destination (`Cmd+Shift+V`).

- **Change permissions** (command palette, Unix): edit owner/group/other
  read/write/execute bits or an octal mode with a before/after preview. Applies
  only to selected objects, including selections from other folders; no recursion
  or Undo. Symlinks and objects without a readable protected handle are rejected.

### Viewing and the rest

- **Basic source syntax colors** for Rust, C/C++, Python and TOML, with a
  **Wrap lines** toggle. Syntax coloring is capped at 64 KiB and disabled under
  Increase Contrast; the ordinary bounded text preview remains available.
  Source/text previews also offer logical line numbers and **Go to line**,
  including wrapped lines; the gutter stays outside selectable source text.
- **Find in preview** supports literal text and an optional **Regex** mode,
  with Match case, Whole words, Prev/Next and Enter/Shift+Enter navigation.
  F3 / Shift+F3 also move to the next/previous match while the search field is
  focused; outside that field F3 keeps its ordinary preview command.
  **Wrap search** defaults on. Turn it off to stop at the first/last collected
  match; unavailable Prev/Next buttons are disabled, and keyboard navigation
  follows the same boundaries. The choice retains the current result.
  Optional **Smart case** ignores case for lowercase queries and matches case
  when the query contains a Unicode uppercase character. Match case overrides
  it. Regex syntax is included in this check; inline regex flags still apply.
  Smart case defaults off and is remembered per pane for the session.
  **Clear** empties the preview search, removes highlights and cancels any
  pending regex job while retaining search options. Focus returns to the query
  field so a new search can be typed immediately.
  Whole words uses Unicode regex word boundaries (underscore and combining
  marks remain parts of words), in both literal and regex mode. Regex indexing
  runs in the background with cancellation and explicit syntax errors; searches
  cover displayed text (including formatted documents and the current PDF page).
- **Formatted JSON preview** preserves key order, duplicate keys, number spellings
  and string escapes while adding indentation. Switch to **Source** for loaded
  original text; both views support find, line numbers and text sizing. Formatting
  is capped at 256 KiB input, 128 nested containers and 1 MiB output; errors fall
  back to source with an explanation. No external tools are required.
- **Text size in previews**: A− / A+ adjust text from 50–200%; 100% resets it.
  Plain text, syntax colors, formatted documents and PDF page text reflow at the
  selected size. Each pane keeps its own session setting; CSV/TSV table view and
  image/PDF raster zoom use their separate controls.
- **Copy displayed text** copies the entire loaded text in the current preview
  representation, including content outside the viewport. Formatted JSON and
  document previews copy rendered text; Source copies the loaded source. Empty
  previews disable the button. CSV/TSV table mode uses Source for this action;
  PDF page text retains its separate copy button.

- **PDF pages on macOS**: Previous/Next, direct page entry, Fit and 25–400%
  zoom of the fixed raster. Reads/renders on preview workers; source limit 64 MiB,
  raster long edge 2048 pixels, two native jobs at once. **Page text** lazily
  extracts the current page for searching and copying (256 KiB cap; no OCR).
  Interactive forms and links use **Open externally**.
- **Interactive image preview**: zoom 25–800% relative to Fit, drag/scroll to
  pan, and double-click or use Fit to reset. Larger zoom requests sharper bounded
  decodes through the existing cache. Rotate left/right by 90° or use **Rotate 180°**, mirror horizontally
  or vertically, and reset orientation without changing the source file.
  Transparent pixels appear over a checkerboard, with White/Black background options.
  **Enlarge small images** lets Fit fill the preview while preserving aspect
  ratio. It defaults off and affects display only; Fit/reset retains the choice,
  while a different file starts with enlargement off.
  **Center image** defaults on; disable it to align smaller images to the upper
  left without changing zoom, orientation or background. Alignment is local to
  the pane and current file; changing it resets pan.
- **Unicode text previews**: automatically decode UTF-16LE/BE and UTF-32LE/BE
  with a BOM, alongside UTF-8. Existing search, line numbers and table/document
  previews work on decoded text; malformed Unicode gets an explicit error.
- **Whitespace markers** in plain/source and JSON text previews: enable
  Whitespace to show spaces as dots and tabs as arrows. Markers follow the
  displayed glyphs without altering text, search offsets or copied content.
  The choice defaults off and is independent in each pane for the session.
- **Formatted Markdown and static HTML** previews, with a Source toggle,
  bounded parsing and no external resource loading or embedded execution.
- **95 bundled palettes** from Strata, selected in View and settings and
  remembered across restarts; system Increase Contrast retains priority.
  See [asset notices](THIRD_PARTY_LICENSES.md).

- **Preview** of images (via ImageIO, including RAW/HEIC) and text in the
  opposite panel, with look-ahead caching. Image/video decoding targets the
  physical preview viewport, applies orientation, and runs through typed
  native/standard fallbacks behind a hard timeout and four-slot limit. Decoded
  images retain dimension and 256 MiB allocation caps; failures are explicit
  and retryable rather than infinite spinners. Text preview is debounced and
  read on an isolated one-worker executor with cancellation, file-identity
  revalidation, a 256 KiB limit, and explicit timeout/error states.
- **Relative dates** in the Modified column (Finder/Things style), with the
  absolute timestamp on hover.
- **Status bar and selection summary**: each panel's footer shows item count and
  total size, and with nothing selected the folder's largest and oldest item;
  selecting swaps in a summary of the selection (count, size, average, kinds,
  largest, oldest).
- **Density tiers** (`Cmd+Shift+D`), a one-shot **focus mode**, rich
  **path-to-clipboard** (`Cmd+Shift+C` and palette variants).
  **Copy name without extension** (palette) copies visible selected names, or the cursor
  name when nothing is selected: `archive.tar.gz` becomes `archive.tar` and
  `.env` stays `.env`. This lexical rule also applies to folder names with dots.
  Multiple names use separate lines, with embedded line controls escaped.
- **Accessible adaptive UI**: active pane, keyboard focus, cursor, selection,
  marks, differences, errors, and disabled state use separate color plus
  shape/text cues. File rows expose named columns and state to assistive
  technology. System high-contrast and reduced-motion preferences are
  honored; text scales from 80% to 200%, with compact toolbars and a bottom
  Operations panel on constrained widths. The bottom key bar is generated
  from the focused pane's available actions, and compact file names preserve
  regular/compound extensions while exposing the full name on hover.
- **Developer diagnostics**: the gear panel shows workers, queued I/O, cache
  bytes/jobs/failures, frame and cancellation percentiles, startup phases,
  persistence recovery, preview-provider fallback/timeouts, watcher
  native/polling policy, merged event batches/reconnects, and CI budgets.
  It can export a capability report or a salted, redacted support bundle and
  exposes bounded rollout/kill controls for optional index, preview, and
  external-provider paths.
- **Native context menu**: Open With, Quick Look, Get Info, Duplicate,
  Compress, Copy Path, Show in Finder, Tags, Share, Move to Trash.
- **Resilient session/config persistence** (panel paths, layout, view toggles,
  bookmarks, smart folders, templates, and collections): a malformed record
  is skipped without discarding valid siblings. Includes a **light / dark
  theme** following the system appearance.

## Repository feature transfers

The [transfer ledger](docs/research/feature-transfers-2026-10-27.md) records
implemented ideas from the 99-repository cohort, source references, validation
and remaining work. The command bar now includes reviewed command recipes plus
editable saved-template names and file-extension filters.

## Strata comparison

The [implementation ledger](docs/strata-parity.md) records transferred features,
existing equivalents, platform differences and remaining work against the pinned
Strata source. Full feature parity is not yet complete.

## Roadmap

Shipped from earlier design rounds: pinned favorites, saved searches,
selection sets, the operation-queue engine, focus mode, compare/diff selection,
relative dates, gather-into-folder, the run-command bar, contextual empty
states (truly empty vs filtered-to-nothing vs permission-denied, each with a
one-click recovery), the transfer-queue panel, vim-style chords, the
marked-files set, per-folder view memory, regex batch-rename, and operation
receipts.

A few scope decisions from the last round, so they don't read as oversights:

- The queue panel's concurrency is fixed at one transfer at a time; the
  progress UI and `active_transfer` state are built around a single job; true
  parallel transfers would need that to become a collection.
- Per-folder view memory and operation receipts are session-lifetime only
  (not written to disk), unlike the current directory's own settings, which
  the session file already persists.
- Operation receipts cover moves, deletes, and batch renames (the operations
  that already raise a completion signal); plain copies aren't logged, since
  they have no undo counterpart in this app.
- Vim chords claim `g`, `s`, `j`, `k` away from type-ahead when they're used
  as chord leaders/motions; mid-search (e.g. typing "backjack") they still
  fall through as ordinary search characters.

The architecture and the prioritised plan for what's next live in
[architecture.md](architecture.md) and [recommendation.md](recommendation.md).
The review material is split by trust level: [audit.md](audit.md) is the
verified defect ranking, [recommendation.md](recommendation.md) now contains a
compact Top-500 cleanup/design backlog, and [backlog.md](backlog.md) keeps the
full 621-item raw sweep. [research.md](research.md) is the external evidence
layer: a fixed, relevant cohort of 100 high-star repositories, 30 primary
papers/standards, and 100 deduplicated proposals. The whole cohort was
revalidated on 2026-07-18 with 100 reachable and zero archived projects. Its
implementation ledger records G001-G050 as the first shipped research
milestone and G051-G100 as the second; H001-H012 and I001-I010 record the two
comparative hardening slices. J001-J010 is shipped (see `research.md`'s Phase 4
ledger): color-managed previews, version-store quota visibility, trust labels,
machine-pressure admission, encrypted support bundles, assistive recovery
timeline notes, change provenance, workspace profiles, conflict rules, and
decoder circuit breakers.

## Review backlog

The 2026-07-09 pass added a compact Top-500 list to
[recommendation.md](recommendation.md): 79 bugs,
194 problems, 115 improvements, and
112 suggestions, all preserving the original
`backlog.md` numbering and locations. [architecture.md](architecture.md) now
mirrors the same work as ten small SOLID/DRY reading slices, so the project can
be understood and refactored one bounded context at a time.

The 2026-07-11 three-pass refresh revalidated all 500 numbers, kept that file
as the single source of truth, and closed the next high-risk interaction
cluster: sync rows now retain stable source paths and directory snapshots,
Batch Rename retains its opening panel/directory/selection, drag-and-drop
requires a visible target and cancels elsewhere, and text diff rejects
quadratic work before allocating its matrix. The native macOS Get Info action
from the earlier pass also remains protected by AppleScript-literal escaping.
The continuation pass adds undo/redo for F2 rename, fixes conflict policies
that can reduce an over-budget transfer, and keeps treemap titles bound to the
directory snapshot they visualize.
The safety pass adds poison-tolerant worker/UI locks, checked thumbnail
allocation, cursor/filter invariants, queue-aware toolbar gating, and complete
Gather folder undo/redo.
The external-research pass samples exactly 100 active repositories across file
managers, editors, search, transfer/backup, storage, Rust desktop foundations,
and keyboard-first tools. Its design conclusion is deliberately conservative:
keep paths and dual-pane browsing visible, enrich them with contextual search,
protect the foreground loop with cancellation and budgets, and define file
operations around recovery and end-to-end completion.
The two implementation milestones now cover all `G001-G100`: the final slice
adds live CI performance probes, percentile telemetry, empirical benchmark
trees, startup phases, developer diagnostics, redacted support exports,
runtime provider controls, KLM workflow budgets, and colocated operation ADRs.
The 2026-07-18 comparative refresh then closed twelve concrete gaps: typed
file/folder comparison, fail-closed conflict policy, item-level settings
recovery, shared command availability with disabled reasons, fully background
and bounded image decoding with retryable failures, and observable watcher
recovery with mandatory reconciliation after gaps or reconnects.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Tab` | Switch active panel |
| `Cmd+T` | New folder tab |
| `Cmd+Shift+T` | Reopen the most recently closed folder tab |
| `Cmd+Shift+W` | Close active folder tab |
| `Cmd+Shift+[` / `Cmd+Shift+]` | Previous / next folder tab |
| `↑` / `↓` | Move cursor |
| `Shift+↑` / `Shift+↓` | Extend selection |
| `Shift+Home` / `Shift+End` / `Shift+PageUp` / `Shift+PageDown` | Extend selection to the top, bottom or by a page |
| `Home` / `End` | Jump to first / last row |
| `PageUp` / `PageDown` | Move by one page |
| `Enter` | Open file / enter folder (`..` row goes up) |
| `Backspace` | Go up one folder (cursor lands on the folder you left) |
| `Cmd+[` / `Cmd+]` | History back / forward |
| `Cmd+1`..`9` | Jump to bookmark slot |
| `Cmd+Shift+1`..`9` | Assign active folder to bookmark slot |
| `Space` | Toggle selection |
| `M` | Toggle mark |
| `j` / `k` | Move cursor down / up (`5j` moves 5 rows) |
| `g g` | Jump to first row |
| `s s` | Reverse sort |
| `Cmd+A` | Select all |
| `Cmd+Shift+I` | Invert selection |
| `Cmd+G` | Select by mask |
| `F2` / `Cmd+R` | Rename |
| `Cmd+Shift+R` | Batch rename |
| `F5` | Copy to the other panel |
| `F6` | Move to the other panel |
| `Cmd+Enter` | Move selection into the highlighted folder |
| `Cmd+Shift+Enter` | Copy selection into the highlighted folder |
| `F7` | New folder |
| `Cmd+Shift+N` | New folder with selection |
| `F8` / `Delete` | Move to Trash |
| `Cmd+Shift+A` / `Cmd+Shift+V` | Add to shelf / drain shelf here |
| `Cmd+D` | Diff selected pair |
| `Cmd+Shift+S` | Synchronize panels |
| `Cmd+Shift+M` | Disk-usage treemap |
| `Cmd+F` | Find files |
| `Cmd+Shift+C` | Copy path |
| `Cmd+L` | Go to path |
| `Cmd+P` | Recent folders |
| `Cmd+E` / `Cmd+U` | Equalize / swap panels |
| `Cmd+I` | Get Info |
| `Cmd+K` | Command palette |
| `Cmd+H` | Toggle hidden files |
| `Cmd+Shift+D` | Cycle list density |
| `F3` | Toggle preview |
| `Cmd+Z` / `Cmd+Shift+Z` | Undo / redo |

More commands (run command on selection, cross-pane diff selection, copy-name /
parent / file-URL / shell / relative path, **copy the listing as text / CSV /
Markdown** (the selection if any, else the whole folder), selection stash and
marked-set algebra, **select clutter files** like `.DS_Store`,
**select the 10 largest**, **files like the cursor**, or **empty files**,
duplicates, saved searches, bookmark this folder, **sort by extension / kind**,
**reverse sort**, the **sort toggles** for folders-first and natural-vs-A-Z
ordering, the **transfer queue** panel, and **operation history**) are
available from the command palette (`Cmd+K`). Names sort naturally (`file2`
before `file10`) by default.

## Build and run

```sh
cargo run --release
```

Requires Rust nightly 1.100 (`rust-toolchain.toml` pins
`nightly-2026-09-11`) and macOS (the app links AppKit / AVFoundation /
ImageIO). See [`CHANGELOG.md`](CHANGELOG.md) for the current package version
and release notes.

### macOS distribution (signing and notarization)

Commander is a native macOS binary. Before shipping outside your own machine:

1. **Sign** the release binary (and any `.app` bundle wrapper) with a Developer
   ID Application certificate, for example
   `codesign --force --options runtime --sign "Developer ID Application: …" path/to/commander`.
2. **Notarize** with Apple (`notarytool submit` against the signed artifact, then
   `stapler staple` for a local ticket) so Gatekeeper accepts downloads.
3. Keep the signed identity, Team ID, and notarization credentials out of the
   repo; CI should inject them from a secrets store. Hardened Runtime is
   required for notarization; grant only the entitlements the app actually uses.

Local `cargo run` builds are unsigned and fine for development. The native
release QA path verifies a running binary's code signature when present; it does
not replace the Developer ID + notarization steps above.

## Development

```sh
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

The file-manager logic lives in a UI-independent core (`workspace`,
`workspace/transfer_queue`, `panel`, `transfer`, `opqueue`, `scan`, `command`,
`compare`, `ui_request`, `workload`, `ports`, `provider_runtime`, `fs_util`,
`rename`, `sync`, `undo`, ...) that is unit-tested without a GUI. `Workspace`
emits typed FIFO `UiRequest` values; `app/update.rs` owns the single per-frame
dispatcher and the `app` module remains the egui adapter. Main-thread AppKit
context-menu behavior and background workload admission are injected through
narrow handles rather than reached through UI-global state.

### Architecture checkpoint

The ordered SOLID/DRY pass completed these ownership boundaries:

- `TransferQueueController`, `DeleteController`, and `SpaceProbeController`
  own their asynchronous lifecycle state; `Workspace` applies typed outcomes.
- `UndoCenter` exclusively owns the undo/redo timeline and replay
  reservations. Async settlement is bound to the center owner, timeline
  revision, history entry, and transfer operation; interrupted replays remain
  locked until the matching recovery is resumed or rolled back.
- `transfer::executor::TransferExecutor` owns the transactional lifecycle from
  preflight through terminal publication. Native/clone, delta, sparse, and
  buffered staging run through replaceable backend ports that cannot place the
  final destination, delete the source, or settle the operation journal.
- `UiState` owns transient input and all dialog/modal buffers while
  `UiRequestQueue` preserves non-modal and modal FIFO ordering.
- `ListingState` owns rows, checked revision, and filter-cache invalidation;
  `ViewState` owns private `ViewConfig` plus bounded per-folder memory.
  Selection, watcher, size index, and pure sorting are separate panel owners.
- `operation_journal` validates stable path identity, legal state transitions,
  restart/migration proofs, and rollback/recovery behavior under injected
  side-effect failures.
- Clipboard, opener, Trash, free-space, and native context-menu behavior use
  typed ports. AppKit selectors return invocation-bound intents and perform no
  filesystem or process effects while the menu is tracking.
- `persistence::Persist` is the shared object-safe byte-store boundary.
  Bookmarks and session state use an injected instance; feature flags and the
  version manifest use the same versioned envelope, bounded no-follow reads,
  generation/revision checks, and fail-closed recovery rules. Recovered
  bookmark sources are quarantined before an explicit upgrade can replace
  them.
- The large workspace integration suite lives in `workspace/tests.rs`;
  pathname parsing/validation lives in its own typed module.
- `path_probe::PathProbeController` debounces `Cmd+L` input, binds every result
  to the exact dialog/input generation, and runs the injected filesystem
  `metadata` probe through a dedicated two-worker workload lane. A dialog owns
  at most two admitted probes and never more than one for its current binding;
  further edits collapse into one latest-wins candidate while older slots
  retire, without growing scheduler freshness state. The dialog keeps stable
  one-line status geometry and a polite accessibility live region.

The full serial suite currently passes 979 tests with three intentional
manual/performance harnesses ignored. Native release QA now records automated
checks and blocks until its permission-bound human attestation is complete.
The next high-value architecture work is moving `PanelState::navigate_to`
listing publication off the UI thread and closing the successful path-probe
TOCTOU window. Durable undo history, path-identity-bound replay, migration of
the operation journal and content index to versioned stores, cross-process
persistence CAS, descriptor-relative filesystem effects, and reconciliation
of the last placement-to-journal crash window remain later schema migrations
or OS-hardening work.

The same checkpoint reduced the locked dependency graph from 559 to 516 crates
by enabling only the image decoders Commander uses. The checked-in
[`deny.toml`](deny.toml) and CI gate for pull requests, main-branch pushes, and
a weekly refresh report zero known vulnerabilities and explicitly accept four
unmaintained advisories in Linux-only upstream UI paths: `RUSTSEC-2026-0192`
for `ttf-parser` in Wayland/winit, plus `RUSTSEC-2026-0370`,
`RUSTSEC-2026-0371`, and `RUSTSEC-2026-0372` for the pre-zbus-6 accessibility
stack. The owner is `@themoretheless`. All four were reviewed on 2026-10-21,
have no safe upgrade in the current eframe/accesskit graph, must be reviewed
again by 2026-11-18, and hard-expire at 00:00 UTC on 2026-12-02. The same review
upgraded `zip` to the patched 9.x line after `RUSTSEC-2026-0367`,
`RUSTSEC-2026-0368`, and `RUSTSEC-2026-0369` were published. CI verifies the
SHA-256 of pinned `cargo-deny` version `0.20.2` before executing it. Duplicate
crate groups remain a warning and tracked dependency debt. Run the same
full-lockfile policy locally with:

```sh
cargo deny --all-features --locked check advisories bans licenses sources
```

### Native visual and release QA

The `visual-qa` feature runs the real native eframe/Glow framebuffer path
against a temporary deterministic workspace. It does not load user session or
storage data, and every native effect is a recording fake. CI runs all twenty-two
scenarios as independent strict matrix jobs:

```sh
cargo run --features visual-qa -- --visual-qa desktop_base --output target/visual-qa
cargo run --features visual-qa -- --visual-qa icons_grid --output target/visual-qa
cargo run --features visual-qa -- --visual-qa image_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa pdf_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa permissions --output target/visual-qa
cargo run --features visual-qa -- --visual-qa rename_presets --output target/visual-qa
cargo run --features visual-qa -- --visual-qa rename_target --output target/visual-qa
cargo run --features visual-qa -- --visual-qa rename_options --output target/visual-qa
cargo run --features visual-qa -- --visual-qa name_list --output target/visual-qa
cargo run --features visual-qa -- --visual-qa checksums --output target/visual-qa
cargo run --features visual-qa -- --visual-qa hex_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa hex_search --output target/visual-qa
cargo run --features visual-qa -- --visual-qa folder_tabs --output target/visual-qa
cargo run --features visual-qa -- --visual-qa code_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa unicode_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa regex_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa json_preview --output target/visual-qa
cargo run --features visual-qa -- --visual-qa archive_browser --output target/visual-qa
cargo run --features visual-qa -- --visual-qa minimum_window --output target/visual-qa
cargo run --features visual-qa -- --visual-qa zoom_200_accessible --output target/visual-qa
cargo run --features visual-qa -- --visual-qa confirmation_owner --output target/visual-qa
cargo run --features visual-qa -- --visual-qa pane_tool --output target/visual-qa
```

Each scenario writes `manifest.json` and `capabilities.json`; a successful
capture also writes `frame.png`. Checks cover framebuffer content, exact
logical viewport size, in-viewport pane/row/dialog geometry, pane separation,
modal ownership, disabled modal background, a pane tool that leaves input live,
painted glyph pixels, and strict native-effect counts. The JSON preview case
requires exactly one recording clipboard write with the displayed-text payload;
all other cases require zero calls. The real clipboard stays untouched.
The 200% scenario opens an `1800x1000` native window so
the application receives the intended `900x500` logical workspace at 2x text
zoom. `--allow-skip` is diagnostic only; CI omits it, so unsupported capture,
timeout, and validation failure are red.

Native release evidence is a separate fail-closed path:

```sh
cargo run --features visual-qa -- --native-release-qa diagnostic --output target/native-release-qa
cargo run --features visual-qa -- --native-release-qa strict --output target/native-release-qa --attestation path/to/attestation.json
```

Diagnostic mode never opens a TCC prompt. `build.rs` embeds the source commit
and dirty bit while watching Git metadata plus every tracked package file.
Runtime evidence ignores `COMMANDER_QA_COMMIT`/`GITHUB_SHA`, verifies that
embedded identity against the exact clean source checkout, and hashes the
current executable with BLAKE3. On macOS it additionally compares the running
process's kernel CDHash with the strictly verified on-disk code signature, so a
replaced `current_exe` path cannot pass as the loaded binary. It also records
the macOS build, existing WindowServer, Accessibility, Screen Recording, and
VoiceOver capability state, full `NSScreen` topology, and a canonical
order-independent fingerprint. It renders the declarative menu into an actual
`NSMenu`, then recursively compares all 22 items and separators: titles, stable
accessibility identifiers, full labels, enabled/state/submenu/shortcut
metadata, targets, selectors, and represented objects. The same pure placement
function used by production verifies that the full top-left-anchored menu
rectangle fits the `visibleFrame` at the center and four corners of every
detected display, including negative coordinates and mixed backing scales.
Oversized menus fail instead of being reported as unclipped.

Speech quality, VoiceOver task navigation, separate AppKit popup pixels,
Escape focus return, and real mixed-display behavior remain permission-bound
human checks. Diagnostic output includes an exact-subject attestation template.
The output directory is private (`0700`), fixed-name JSON files are atomically
replaced through a held no-follow directory descriptor, and files are `0600`;
symlink, non-regular, oversized, corrupt, and unknown-field input fails closed.
Evidence schema v4 and attestation schema v2 bind strict mode, commit,
executable digest, topology, case set, reviewer, notes, and timestamp. The
checked-in structural references are
[`qa/native-release-attestation.schema.json`](qa/native-release-attestation.schema.json)
and
[`qa/native-release-attestation.template.json`](qa/native-release-attestation.template.json).
Strict mode accepts only a clean exact commit/binary/topology binding, all
native capabilities, all automated checks, and all five human cases passed
within seven days. Denied, blocked, `not_run`, stale, mismatched, malformed, or
missing evidence exits nonzero.

On the 2026-07-28 implementation host, actual 22-item NSMenu introspection and
all 15 full-rectangle placement probes passed on three displays (1x at negative
x, 2x main, and 2x above). Accessibility and Screen Recording were denied and
VoiceOver was not running, so evidence remained non-passing and strict mode
returned nonzero. No permission prompt was shown.

The production WGPU presentation path remains in the normal application. It is
not used for automated readback: on the tested Metal host, eframe 0.35 queued
`ViewportCommand::Screenshot` but did not deliver `Event::Screenshot` without
external device polling, while polling outside the renderer's event-loop
ownership could hang. Such a timeout is classified as a capture failure, never
as an unsupported capability.

Manual release check:

- Complete the primary two-pane journey with VoiceOver.
- Invoke the focused row's AccessKit `ShowContextMenu` action with
  VoiceOver-Shift-M, including a row in the inactive pane, then navigate the
  resulting AppKit menu without a pointer.
- Verify separate popup pixels and all submenus at display edges.
- Dismiss with Escape and verify focus returns to the exact row.
- Repeat placement on the attested mixed 1x/2x topology.

The architecture and the refactoring plan are documented in
[architecture.md](architecture.md) and [recommendation.md](recommendation.md).

## License

MIT. See [LICENSE](LICENSE).
