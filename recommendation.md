# Recommendations

The prioritised plan of what to do next, kept in sync with
[architecture.md](architecture.md) (the target shape),
[README.md](README.md) (user-facing capabilities), and
[audit.md](audit.md) (the ranked list of concrete defects).

## Strategy

`master` is the mainline. The `refactor/god-removal-ui-state` branch is treated
as a **proven spike**, not a merge candidate: its ideas (Effect bus, `UiState`,
`Pane`/`CommandHandler` traits, tokio async) are re-landed onto `master` in
small, reviewable steps. It diverged on a stale base (`master` ~42 commits
ahead, the spike ~13), so a wholesale merge would be conflict-heavy and is
explicitly out of scope. Where a step below has a working reference on the
spike, that is noted as "mine from spike".

Two tracks run in parallel: **Track A** decomposes the two god objects
(architecture work); **Track B** fills the gaps README still lists as unbuilt
(feature work). They intersect at `ViewConfig` (A3 unblocks B2).

## Track A - architecture re-land sequence

Each step is one green commit (`cargo test` + `clippy` + `fmt`), ordered by
risk. The keystone (A4) is deliberately not first: its payoff lives in
egui-frame focus behaviour the 373 tests cannot guard, so it must be verified by
hand in the running app.

| # | Step | Risk | Notes |
| --- | --- | --- | --- |
| A1 | Extract `compare` module | done | Shipped on `master` (`ddab764`). |
| A2 | Extract `pathname` validators (`resolve_dir_input`, `validate_new_name`); move `workspace.rs`'s ~1,300-line test module into `workspace/tests.rs` | low | Pure file moves, compiler-verified. |
| A3 | Introduce `ViewConfig` value object (sort/filter/hidden + `sort_entries`) | low-med | Covered by existing sort tests. Unblocks B2. |
| A4 | Replace the `*_request` flag bus with one typed `effects: Vec<Effect>` queue | **med** | Keystone. Mine from spike (`process_effects`, `Requests` removed). Preserve the dialog focus edge-trigger; verify manually in-app. |
| A5 | Extract `UiState` (group the ~20 dialog buffers out of `App`) | med | Mine from spike. Shrinks the `App` god object. |
| A6 | Extract `TransferCenter` + `UndoCenter` from `Workspace` | med | Delegation; behaviour-preserving. |
| A7 | Define and inject `Clipboard` / `Trash` / `Persist` ports; return a structured `OpOutcome` | med | Completes the hexagon; makes the core side-effect-free in tests. |

Deferred from the spike (re-land only on explicit demand, each is a feature in
its own right, not cleanup): tokio runtime + `spawn_blocking`, virtualised file
list, tabs, git status column, tags, notes, configurable columns.

## Track B - unbuilt features from the README roadmap

Verified against the code (statuses are real, not the README's stale list):

| # | Feature | Status today | Priority | Effort |
| --- | --- | --- | --- | --- |
| B1 | Regex find/replace in batch-rename | absent | **high** | small |
| B2 | Per-folder view memory (sort/filter/hidden/density) | absent | high | medium |
| B3 | Queue panel UI (pause/resume/reorder/concurrency) | partial (engine done) | medium | medium |
| B4 | Marked-files set distinct from the cursor selection | partial (manual stash only) | medium | medium |
| B5 | Operation receipts (searchable history + jump-back) | absent | low | large |
| B6 | Vim-style key chords (`5j`, `gg`, `ss`) | absent | low | medium |

Sequencing rationale:

- **B1 (regex rename) first.** Pure core, no UI plumbing: add the `regex`
  crate, a `use_regex` flag on `RenameRule`, regex replace in `rename.rs`, a
  toggle in `batch_rename_dialog`. Fully unit-testable, high user value, no
  dependency on Track A.
- **B2 (per-folder view memory) rides on A3.** Once `ViewConfig` exists, add a
  `HashMap<PathBuf, ViewConfig>`, look it up in `navigate_to`/`go_up`/
  `go_back`/`go_forward`, capture changes, and persist via `session`. Do B2
  immediately after A3.
- **B3 (queue panel) pairs with A4/A6.** The engine (`opqueue`) already
  supports pause/resume/reorder/`set_concurrency`; what is missing is the panel
  UI, the queue commands, and relaxing the input-gate in `app/keys.rs` that
  blocks queuing a second transfer. Cleaner to build once the Effect bus and
  `TransferCenter` exist.
- **B4 (marked set)** needs a new per-panel marked field (analogous to
  `selected`), a mark/unmark toggle, visual rendering, preservation across
  navigation, and integration with the existing stash algebra. Not on the spike.
- **B5 (receipts)** is the largest: a persistent, searchable operation log with
  jump-back, distinct from the in-memory undo stack. Defer.
- **B6 (vim chords)** needs a multi-frame chord state machine (leader buffer +
  count prefix + timeout) that does not exist today and is not on the spike.

## Track C - documentation hygiene (do now, trivial)

From the README-vs-code audit:

- **Roadmap is stale: "Contextual empty states" is already shipped.**
  `app/file_list.rs` distinguishes filtered-to-nothing / permission-denied /
  vanished / empty, each with a one-click recovery. Move it from "What remains"
  to the shipped list; the roadmap drops from 7 items to 6.
- **Terminology drift:** the README says "Reveal in Finder" but the native menu
  item is labelled "Show in Finder" (`native_menu.rs`). Align the README.
- Optionally name the `compare` module in the Development module list.

The rest of the README is accurate: the Features bullets, the keyboard table,
and the module list all matched the code in the audit (zero other drift).

## Track D - correctness fixes from the audit

The full ranked list of 50 defects (with manual-verification notes) is in
[audit.md](audit.md), refreshed after round 2 and with already-fixed items
removed. After verification, the items worth scheduling, in priority order
(ranks reference the **round-2** audit table):

| # | Fix | round-2 rank | Effort | Status |
| --- | --- | --- | --- | --- |
| D1 | Surface save failures: check `write_atomic`'s return and toast | (was r1 #7) | small | **done** (`177e67c`) |
| D2 | Stop swallowing `Result` in the undo/redo apply path; toast | (was r1 #20) | small | **done** (`177e67c`) |
| D3 | Evict the image cache on directory change (honour the docstring) | (was r1 #2) | trivial | **done** (`177e67c`) |
| D4 | Cheap per-frame perf: clone `FontId` once per row; `HashSet`-back the shelf; pre-lowercase `NameContains`; avoid the per-call/per-keystroke `filtered_entries` Vec | 34, 35, 15, 16, 36, 37 | small | open |
| D5 | Make the dir-size index race-safe (generation counter or staged swap), drop the redundant nested `install()`, and bound `walk_log`/`dir_size_cache` | 17, 19, 20, 21, 38, 39 | medium; pairs with the `DirIndex` extraction | open |
| D6 | Fix reachable panics and overflow: `lock().unwrap()` poisoning, ObjC `unwrap`, `batch_rename` unwrap, unchecked `keep[gi]`, `checked_mul` the thumbnail buffers | 3, 24, 29, 13, 10, 4, 2 | small | open |
| D8 | Rename temp-name correctness: homogenise the reserved-set casing and make the rollback composite-error / atomic | 9, 8 | medium; one pass with tests | open |
| D9 | Destructive-op partial-failure integrity: consistent `path_is_taken` + no-clobber swap, fail-loud partial undo of Move, propagate `copy_symlink`/`cleanup_path` errors, roll back the orphan gather, and add the on-disk undo round-trip test | 1, 25, 11, 12, 46, 7, 14 | medium; with integration tests | open |
| D10 | Panel filter/cursor invariants: `ensure_cursor_valid()` after every filter/facet/sort change, bounds-checked `filtered_entries`, and an explicit (not silent-empty) `selected_or_cursor` miss | 5, 6, 33 | small; strongest case for the `ViewState` encapsulation in Track A | open |
| D11 | egui widget-Id hygiene: add `id_salt` to the three dialog `ScrollArea`s and derive toast Ids from stable identity | 26, 27, 28, 41 | trivial | open |

Severity caveats from manual verification (do not act on these as written):
round-2 **#1** is a real `exists()`/`path_is_taken()` inconsistency but the
"overwrites the symlink target" framing is wrong (`rename` replaces a broken
symlink atomically); round-2 **#2** needs CoreGraphics to actually decode a
pathological image, so treat `checked_mul` as defense-in-depth; round-2 **#17**
is a redundant nested `install()`, not a proven deadlock; round-2 **#18** is a
fragile-but-not-live pattern (`copyfile` is synchronous). See the corrections
table in [audit.md](audit.md).

## Tracking

Done: **C** (docs), and **D1 + D2 + D3** (commit `177e67c`): save writers return
their success bool and the explicit-save dialogs toast on failure; undo/redo
surface a refused rename instead of swallowing it; the image cache is flushed on
directory change.

Next suggested order: **D6 (panics/overflow) + D10 (cursor invariants) first**
(both small, both close reachable crashes that round 2 surfaced), then **B1
(regex) -> A2 -> A3 -> B2 -> A4 ...**, with D4 (cheap perf) and D11 (id_salt)
slotted in as low-risk fillers. Schedule **D9** (destructive-op partial-failure
integrity, with on-disk undo tests) as a dedicated pass, and **D5** alongside the
`DirIndex` extraction. D10 is also the strongest concrete motivation for the
`ViewState` encapsulation in Track A.
