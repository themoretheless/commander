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
  folders), type-ahead jump, and a per-panel filter box with quick-filter
  facets (kind, size, and date buckets: Today / Week / Month, plus an
  Older-than-a-month bucket).
- **Command palette** (`Cmd+K`): fuzzy-filter every command, ranked by recency
  and frequency.
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
  engine as the keyboard.
- **Gather into a new subfolder** (`Cmd+Shift+N`): move the selection into a
  freshly-named folder in one undoable step (Finder's New Folder with
  Selection).
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
A ranked, verified list of concrete defects is in [audit.md](audit.md); a much
wider, unverified single-pass inventory (621 bugs/problems/improvements/
suggestions from a file-by-file sweep) is in [backlog.md](backlog.md).

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
