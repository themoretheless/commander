# Commander

A dual-pane file manager for macOS, built in Rust with [egui](https://github.com/emilk/egui).

Two side-by-side panels, keyboard-first navigation, and native macOS
integration (Quick Look, Finder tags, share sheet, APFS clone copies).

The file-manager logic lives in a UI-independent, unit-tested core; the `app`
module is a thin egui layer over it.

## Features

### Panels and navigation

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
  and frequency.
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

- **Copy / move / delete** on a background thread with a live progress window
  (speed graph, ETA, per-file error list). Copies use native `copyfile` with
  APFS cloning and fall back to a buffered copy; a same-volume move is an
  instant atomic rename.
- **Durable operations**: Fast/Verified/Versioned profiles, typed failures,
  source/destination revalidation, an idempotent operation journal, safe-state
  lockout, and a Recovery Center with Resume, Roll back, Inspect, and repair
  plans.
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
- **Drag and drop** between panels and onto subfolders, routed through the same
  engine as the keyboard. `Cmd+Enter` moves into the highlighted folder and
  `Cmd+Shift+Enter` copies into it; both actions are also available from the
  toolbar overflow menu for single-pointer use.
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
  relative-size occupancy bars.
- **Cross-pane selection**: select files only in this panel, differing from the
  other, identical to the other, or same-named (palette).
- **Selection algebra**: select-by-mask (`Cmd+G`), and stash/union/intersect/
  subtract/symmetric-difference of selections.
- **Marked-files set** (`M` to toggle), distinct from the interactive
  selection and never cleared by select-all/invert/clear-selection, with its
  own union/intersect/subtract/symmetric-difference against the selection
  (palette).
- **Drop-stack shelf**: gather files across folders (`Cmd+Shift+A`) and drain
  them into one destination (`Cmd+Shift+V`).

### Viewing and the rest

- **Preview** of images (via ImageIO, including RAW/HEIC) and text in the
  opposite panel, with look-ahead caching.
- **Relative dates** in the Modified column (Finder/Things style), with the
  absolute timestamp on hover.
- **Status bar and selection summary**: each panel's footer shows item count and
  total size, and with nothing selected the folder's largest and oldest item;
  selecting swaps in a summary of the selection (count, size, average, kinds,
  largest, oldest).
- **Density tiers** (`Cmd+Shift+D`), a one-shot **focus mode**, rich
  **path-to-clipboard** (`Cmd+Shift+C` and palette variants).
- **Accessible adaptive UI**: active pane, keyboard focus, cursor, selection,
  marks, differences, errors, and disabled state use separate color plus
  shape/text cues. File rows expose named columns and state to assistive
  technology. System high-contrast and reduced-motion preferences are
  honored; text scales from 80% to 200%, with compact toolbars and a bottom
  Operations Center on constrained widths.
- **Native context menu**: Open With, Quick Look, Get Info, Duplicate,
  Compress, Copy Path, Show in Finder, Tags, Share, Move to Trash.
- **Session persistence** (panel paths, layout, view toggles) and a **light /
  dark theme** following the system appearance.

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
layer: 100 high-star repositories, 30 primary papers/standards, and 100
deduplicated proposals. Its implementation ledger records G001-G050 as the
first shipped research milestone, G051-G090 as implemented slices of the
second, and G091-G100 as the remaining sequential work.

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

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| `Tab` | Switch active panel |
| `↑` / `↓` | Move cursor |
| `Shift+↑` / `Shift+↓` | Extend selection |
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

Requires a recent stable Rust toolchain and macOS (the app links
AppKit / AVFoundation / ImageIO).

## Development

```sh
cargo test                       # unit tests (UI-independent core)
cargo clippy --all-targets       # lints (the repo is clippy-clean)
cargo fmt --check                # formatting
```

The file-manager logic lives in a UI-independent core (`workspace`, `panel`,
`transfer`, `opqueue`, `scan`, `command`, `compare`, `fs_util`, `rename`,
`sync`, `bookmarks`, `jumplist`, `cmdtemplate`, `undo`, `receipts`, ...) that
is unit-tested without a GUI; the `app` module is a thin egui layer over it.
The architecture and the refactoring plan are documented in
[architecture.md](architecture.md) and [recommendation.md](recommendation.md).

## License

MIT. See [LICENSE](LICENSE).
