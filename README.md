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
- **Marked-files set** (`M` to toggle), distinct from the interactive
  selection and never cleared by select-all/invert/clear-selection, with its
  own union/intersect/subtract/symmetric-difference against the selection
  (palette).
- **Drop-stack shelf**: gather files across folders (`Cmd+Shift+A`) and drain
  them into one destination (`Cmd+Shift+V`).

### Viewing and the rest

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
- **Accessible adaptive UI**: active pane, keyboard focus, cursor, selection,
  marks, differences, errors, and disabled state use separate color plus
  shape/text cues. File rows expose named columns and state to assistive
  technology. System high-contrast and reduced-motion preferences are
  honored; text scales from 80% to 200%, with compact toolbars and a bottom
  Operations Center on constrained widths. The bottom key bar is generated
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
comparative hardening slices. J001-J010 is the current unimplemented idea set.

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

Requires Rust nightly 1.100 (`rust-toolchain.toml` pins
`nightly-2026-09-11`) and macOS (the app links AppKit / AVFoundation /
ImageIO).

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
a weekly refresh report zero known vulnerabilities and explicitly accept one
unmaintained advisory, `RUSTSEC-2026-0192`, for `ttf-parser` in the Linux
Wayland/winit stack. The waiver is owned by
`@themoretheless`, its review is due on 2026-10-21, and it hard-expires at
00:00 UTC on 2026-10-28. CI verifies the SHA-256 of pinned `cargo-deny` version
`0.20.2` before executing it. Forty-one duplicate-crate groups remain a warning
and tracked dependency debt. Run the same full-lockfile policy locally with:

```sh
cargo deny --all-features --locked check advisories bans licenses sources
```

### Native visual and release QA

The `visual-qa` feature runs the real native eframe/Glow framebuffer path
against a temporary deterministic workspace. It does not load user session or
storage data, and every native effect is a recording fake. CI runs all four
scenarios as independent strict matrix jobs:

```sh
cargo run --features visual-qa -- --visual-qa desktop_base --output target/visual-qa
cargo run --features visual-qa -- --visual-qa minimum_window --output target/visual-qa
cargo run --features visual-qa -- --visual-qa zoom_200_accessible --output target/visual-qa
cargo run --features visual-qa -- --visual-qa confirmation_owner --output target/visual-qa
```

Each scenario writes `manifest.json` and `capabilities.json`; a successful
capture also writes `frame.png`. Checks cover framebuffer content, exact
logical viewport size, in-viewport pane/row/dialog geometry, pane separation,
modal ownership, disabled modal background, painted glyph pixels, and zero
native-effect calls. The 200% scenario opens an `1800x1000` native window so
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
