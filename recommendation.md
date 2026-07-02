# Recommendations

The prioritised plan of what to do next, kept in sync with
[architecture.md](architecture.md) (the target shape),
[README.md](README.md) (user-facing capabilities),
[audit.md](audit.md) (the ranked, verified list of concrete defects), and
[backlog.md](backlog.md) (a wider, unverified single-pass inventory of 621
smaller bugs/problems/improvements/suggestions from a file-by-file sweep of
the whole codebase - nothing in it is scheduled; treat it as a source to pull
from, not a plan).

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
egui-frame focus behaviour the 375 tests cannot guard, so it must be verified by
hand in the running app.

The table below is now a **high-level index only**. A dedicated design pass
(3 independent architects, a synthesis, then 3 rounds of adversarial critique
against SOLID-compliance/DRY/"reviewable in isolation" lenses) turned this
into a fully detailed **33-step, 75-module** decomposition - see
[architecture.md's "SOLID/DRY module decomposition"](architecture.md#solid-dry-module-decomposition-design-pass-3-critiquerefine-iterations)
section for the exact module list, dependency graph, migration-step-by-step
detail (with file:line citations), suggested reading order, and the risks the
design pass itself flagged. **That section is the authoritative plan**; the
A1-A7 table here is a stable quick index onto it (each row points at the
detailed steps), kept because A1-A7 are the names everything else in this doc
references. When the two disagree, architecture.md wins.

| # | Step | Risk | Notes |
| --- | --- | --- | --- |
| A1 | Extract `compare` module | done | Shipped on `master` (`ddab764`). |
| A2 | Move `workspace.rs`'s test module to `workspace/tests.rs` (Step 0), then extract `pathname` (Step 1) | low | Pure file moves, compiler-verified. Detailed as architecture.md's Steps 0-1. |
| A3 | Introduce `ViewConfig` value object (sort/filter/hidden + `sort_entries`) | low-med | Covered by existing sort tests. Unblocks B2. Now Steps 3-9 of the detailed plan (panel leaves extracted first, `ViewConfig` and its `filter_cache` sibling-fix land together at Steps 7-8 since they share one invariant). |
| A4 | Replace the `*_request` flag bus with one typed `effects: Vec<Effect>` queue | **med** | Keystone. Mine from spike (`process_effects`, `Requests` removed). Preserve the dialog focus edge-trigger; verify manually in-app. Split into Steps 14-16 (a `DialogKind` leaf first, then an additive `effects` field, then the risky cutover alone) plus the UI-side drain in Step 29. |
| A5 | Extract `UiState` (group the ~20 dialog buffers out of `App`) | med | Mine from spike. Shrinks the `App` god object. Split into Steps 16-17 (`dialog_state_types` then `dialog_buffers`); D19's dialog-retargeting fix lands in the same commit as Step 17 since both touch the same lines. |
| A6 | Extract `TransferCenter` + `UndoCenter` from `Workspace` | med | Delegation; behaviour-preserving. Steps 13, 18-21 - `TransferCenter` takes a narrow `Refreshable` trait rather than `&mut PanelState` (an ISP fix the critique required), and `workspace::transfer_requests` splits off as a peer module for pure plan computation. |
| A7 | Define and inject `Clipboard` / `Trash` / `Persist` ports; return a structured `OpOutcome` | med | Completes the hexagon; makes the core side-effect-free in tests. Steps 26-28 - note `Persist` grew to three helpers (`load_lenient`, `load_optional` for `session.rs`'s no-`Default` case, `save_atomic`) once the design pass checked every call site's actual signature, and D12's AppleScript-injection fix is a plain escaping function, not a port (the call site is a static `extern "C"` callback with no `self` to inject one onto). |

Deferred from the spike (re-land only on explicit demand, each is a feature in
its own right, not cleanup): tokio runtime + `spawn_blocking`, virtualised file
list, tabs, git status column, tags, notes, configurable columns. Two more
splits are in the detailed plan as **optional, beyond committed Track A**
(Steps 31-32): `command.rs`'s palette-ranking machinery into `palette.rs`, and
`app/confirm_dialog.rs`'s two list-rendering strategies into their own files -
land only if reviewers want them after A1-A7 lands clean.

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

From the round-1 README-vs-code audit (all **done**, landed in `9b306f0`):

- ~~Roadmap was stale: "Contextual empty states" already shipped.~~ Moved to
  the shipped list.
- ~~Terminology drift: README said "Reveal in Finder", native menu says "Show
  in Finder".~~ README aligned.
- ~~Name the `compare` module in the Development module list.~~ Added.

From the round-3 docs-vs-code audit (**done** in this pass):

- `architecture.md` said "373 GUI-free tests" in three places; `cargo test`
  reports 375 passing (376 `#[test]` functions, 1 `#[ignore]`d profiling
  harness at `panel.rs:2519`). Fixed.
- `architecture.md`'s module map omitted `toasts` (`src/toasts.rs`), a
  genuinely UI-independent, unit-tested module (no egui types) that has
  existed since the toast system landed. Added to the "Presentation-independent
  helpers" list.

The rest of README.md is still accurate as of round 3: the Features bullets,
the keyboard table, and the module list all match the code (zero other
drift). No README changes were needed this round.

## Track D - correctness fixes from the audit

The full ranked list of 50 defects (with manual-verification notes) is in
[audit.md](audit.md), now refreshed to **round 4** (8 more area-scoped
reviewers: the rest of `workspace.rs` line-by-line, a fresh `panel.rs` pass,
undo/redo completeness, keybinding/command consistency, filesystem edge
cases, a test-coverage-gap hunt, and the `app/mod.rs`/`render.rs`/
`file_list.rs` rendering layer), with 16 new confirmed/plausible defects
merged in and 16 items pushed below the cut. Ranks below reference the
**round-4** audit table; items no longer in the numbered table are cited as
"(below the cut)", matching audit.md's plain-list convention (rank numbers are
not carried across rounds since the synthesis re-ranks everything each time).
(The D-numbering skips D7: it was a round-2 item folded into D6/D8 during an
earlier refresh and the label was retired rather than reused, so the gap is
intentional, not a dropped row.)

| # | Fix | round-4 rank(s) | Effort | Status |
| --- | --- | --- | --- | --- |
| D1 | Surface save failures: check `write_atomic`'s return and toast | (was r1 #7) | small | **done** (`177e67c`) |
| D2 | Stop swallowing `Result` in the undo/redo apply path; toast | (was r1 #20) | small | **done** (`177e67c`) |
| D3 | Evict the image cache on directory change (honour the docstring) | (was r1 #2) | trivial | **done** (`177e67c`) |
| D4 | Cheap per-frame perf: clone `FontId` once per row; `HashSet`-back the shelf; pre-lowercase `NameContains`; avoid the per-call/per-keystroke `filtered_entries` Vec | 24, 25, below the cut (x3) | small | open |
| D5 | Make the dir-size index race-safe (generation counter or staged swap), drop the redundant nested `install()`, and bound `walk_log`/`dir_size_cache` | 26, 45, below the cut (x3) | medium; pairs with the `DirIndex` extraction | open |
| D6 | Fix reachable panics and overflow: `lock().unwrap()` poisoning (transfer, image_cache/confirm_dialog, and the `copyfile` C callback), ObjC `unwrap`, `batch_rename` unwrap, unchecked `keep[gi]`, `checked_mul` the thumbnail buffers | 9, 10, 11, 12, 21, below the cut (x2) | small | open |
| D8 | Rename temp-name correctness: homogenise the reserved-set casing and make the rollback composite-error / atomic | 15, 16 | medium; one pass with tests | open |
| D9 | Destructive-op partial-failure integrity: consistent `path_is_taken` + no-clobber swap, fail-loud partial undo of Move, propagate `copy_symlink`/`cleanup_path` errors, roll back the orphan gather, add rollback to `commit_rename`'s case-only path, and add the on-disk undo round-trip test | 13, 14, 20, 22, 34, 23, below the cut | medium; with integration tests | open |
| D10 | Panel filter/cursor invariants: `ensure_cursor_valid()` after every filter/facet/sort change, bounds-checked `filtered_entries`, and an explicit (not silent-empty) `selected_or_cursor` miss | 18, 19, below the cut | small; strongest case for the `ViewState` encapsulation in Track A | open |
| D11 | egui widget-Id hygiene: add `id_salt` to the three dialog `ScrollArea`s and derive toast Ids from stable identity | below the cut (x4) | trivial | open |
| D12 | **Security: escape or eliminate the AppleScript injection in `action_get_info`** (interpolated filename breaks out of the AppleScript string literal into `do shell script`) | 1 | small; escape `"`/`\` or drop the AppleScript call for a native `NSWorkspace`/Finder API | open, **do first** |
| D13 | Cross-pane comparison directory-blindness: add `is_dir` checks to `sync::compare`/`compare::classify_entry`/`conflict::detect`, and key `apply_sync`'s name-collision resolution by path/index instead of lowercased name | 2, 27, 28 | medium | open |
| D14 | Cap `textdiff`'s line count (or switch to a linear-space diff) before the O(n·m) DP allocation, so two ordinary text files can't abort the process | 5 | small | open |
| D15 | Data-safety gating: require an explicit drop-target (or a confirmation) before `drop_dragged` falls back to Move-into-other-panel, and gate toolbar Copy/Move/Delete on `pending_op`/`active_transfer` like the keyboard and drag-drop paths already do | 6, 32 | small-medium | open |
| D16 | Image pipeline: shrink the preload window by remaining cache budget instead of a hardcoded floor of 50, cap concurrent decode threads, add a negative-cache for undecodable formats (SVG/MKV/WebM), and apply EXIF/HEIF orientation | 17, 36, 37, 38 | medium | open |
| D17 | Persistence hardening: bound `MaxAgeDays`/`MinAgeDays` (or use `checked_mul`/`saturating_mul`), and give the four config-store loaders item-level fault tolerance instead of discarding the whole file on one bad field | 29, 31 | small-medium | open |
| D18 | Small UI/data-integrity fixes: `select_all` should preserve filtered-out selections like `invert_selection` does; run Find's directory walk off the UI thread; clear the batch-rename dialog's stale error on rule edit; scope `Escape` to the active panel's preview only | 33, 35, 49, below the cut | small each | open |
| D19 | **Non-modal dialog retargeting: snapshot the working panel/selection/directory once at dialog-open time** instead of re-deriving it live from `Workspace` every frame, for the batch-rename studio and the treemap dialog | 3, 42 | medium; natural fit for the `UiState` extraction (A5) | open |
| D20 | Undo coverage gaps: add a `Rename` variant to `undo::Action` so F2 single-file rename is undoable (and toast when an action genuinely can't be undone, instead of silently reverting something else or no-op'ing); make "Gather into Folder"'s undo also remove the now-empty folder it created | 8, below the cut | medium | open |
| D21 | Conflict-resolution UI deadlock: recompute `need_bytes`/`overflow` after `resolve_pending_conflicts` shrinks `tr.entries`, so a chosen policy (Skip Existing, Keep Newer, ...) can actually un-stick the disabled buttons it was meant to fix | 7 | small-medium | open |
| D22 | Drag-and-drop plumbing rewrite: capture the actual dragged row(s) explicitly instead of falling back to a stale `panel.selected` when the drag starts on an unselected row; mirror drag state so the destination panel can render its own drop-target highlight; clear `drag_entries`/`drop_target` on `drop_dragged`'s early-return concurrency guard instead of leaving a phantom overlay | 4, 43, 44 | medium; one rewrite closes all three plus the already-tracked #6 | open |
| D23 | Filesystem edge-case hardening: run `free_space()`'s `df` call off the UI thread with a timeout; make `copy_dir_all` handle a directory symlink the way `transfer.rs`'s `copy_dir_buffered` already does; don't delete a whole partially-copied destination tree over one `copy_dir_native` file error; add an `ENOTSUP` fallback to `rename_noreplace` | 39, 40, 41, 50 | medium | open |
| D24 | Silent no-op cleanup: toast when `JumpSlot` targets a missing directory; toast on copy-path commands with an empty selection; give `JumpList` a way to prune a dead entry instead of only bypassing it; fix `select_by_mask`'s live-count preview to agree with what Select will actually do for a subtraction-only mask | 46, 47, below the cut (x2) | small each | open |

Severity caveats from manual verification (do not act on these as written):
round-4 **#20** (was round-2 #1) is a real `exists()`/`path_is_taken()`
inconsistency but the "overwrites the symlink target" framing is wrong
(`rename` replaces a broken symlink atomically); round-4 **#21** (was
round-2 #2) needs CoreGraphics to actually decode a pathological image, so
treat `checked_mul` as defense-in-depth; round-4 **#26** (was round-2 #17) is
a redundant nested `install()`, not a proven deadlock; the below-the-cut
`copyfile`-callback items are fragile-but-not-live patterns (`copyfile` is
synchronous); round-4 **#48** (`opqueue` pause/complete stranding) is a real
transition-table gap but currently dead code (no live caller), so treat it as
pre-work for B3, not an active bug; round-4 **#36** (missing negative-cache for
undecodable image formats) is real but bounded by fast decode-attempt
failures, not the unbounded-memory class its initial "high" framing suggested;
round-4 **#46** (jump-list never validates a target exists) looked like a
permanent trap but the existing "Go up" affordance on the Gone-state screen
already escapes it each time, so it's a recurring papercut, not a dead end;
round-4 **#50** (`rename_noreplace` `ENOTSUP` fallback) rests on an honestly
un-reproduced OS behavior (no exotic filesystem was available to test against)
- real gap, narrow trigger. See the corrections tables in
[audit.md](audit.md).

## Track E - ideas backlog (unscoped, not yet prioritised)

A round-4 brainstorm from 5 angles (competitive gap analysis, power-user
workflow, architecture, reliability/data-safety, scale/performance) surfaced
30 ideas that are not bugs and not yet on the roadmap. These are a parking
lot, not a commitment - nothing here is scheduled. Promote an idea into
Track A/B by giving it a letter once it's actually prioritised.

**Feature ideas (from competitive-gap and power-user-workflow angles):**

| Idea | Effort | Note |
| --- | --- | --- |
| Archive browsing/extraction (list a .zip/.tar.gz as a pseudo-folder, extract selected entries) | large | The one clear asymmetry vs. every competitor: the app can compress but not decompress/browse |
| Expose the existing `content_hash`/dedup hashing as a user-facing checksum/verify command | small | Plumbing already exists internally for dedup |
| Symlink/alias/hardlink creation from the selection | small | Only "Copy Path as text" exists today, no "make a link here" |
| Synchronized dual-pane navigation lock (distinct from the existing one-shot Sync sheet) | medium | For parallel tree browsing (source/build, or two snapshots) |
| Format-specific extra columns (image dimensions, audio duration) shown for free using the already-paid-for ImageIO decode | medium | Narrower than the deferred general "configurable columns" |
| Capture and show run-command output (stdout/stderr/exit code) instead of fire-and-forget spawn | medium | The run bar currently gives zero feedback beyond "started" |
| Per-template working-directory and foreground/background flag on `cmdtemplate::Template` | small | Small typed addition to an already-reusable templating engine |
| "Repeat last command" / dot-repeat binding | small | `command.rs` already tracks `UsageStats`; distinct from B6's vim chords |
| Palette entries for named bookmarks/recents beyond the 9 numbered slots | medium | Named bookmarks beyond slot 9 are currently mouse-only |
| CLI launch args (`commander <left> [right]`) for a terminal-to-GUI handoff | small | No `env::args()` handling exists today |
| Export/import command templates and keymap as shareable dotfiles | small | `cmdtemplate` already round-trips through serde_json |

**Reliability / data-safety ideas** (the product-level answer to the audit's
whole "destructive-op partial failure" theme, rather than fixing one code path
at a time):

| Idea | Effort | Note |
| --- | --- | --- |
| Post-copy size/checksum verification with one-click re-copy of just the failed files | medium | Reuses the existing `content_hash` primitive |
| Append-only crash-survivable operation journal, with a "resume cleanup" dialog on next launch | large | Distinct from B5 (receipts are UX/history; this is crash recovery for operations that never finished) |
| Dry-run/preview step for Sync and large batch Delete/Move | medium | Sync can delete destination-only files; today the only inspection surface is the tinted row list |
| Route Delete-to-Trash undo through the same `UndoStack` as Move/Rename | small | `trash_paths`'s own doc comment admits deletes aren't undoable today |
| Pre-flight collision/permission/path-length scan before a transfer starts, not discovered file-by-file mid-transfer | medium | Reuses the walk the free-space preflight already does |
| Route move/overwrite cleanup removals through Trash (or a quarantine dir) instead of a hard `remove_file`/`remove_dir_all` | medium | Today only explicit Delete goes through Trash; implicit removals inside Move/overwrite don't |

**Architecture ideas (beyond the already-planned Track A):**

| Idea | Effort | Note |
| --- | --- | --- |
| Panic containment on the three raw background threads (`catch_unwind` or switch to `parking_lot::Mutex`, which doesn't poison) | small | A single background panic today can poison a Mutex and cascade into a full app crash |
| A structured logging facade (the `log` crate + a file-backed subscriber) | small | Currently one `eprintln!` in the whole tree; background failures leave no durable trail |
| A headless `egui_kittest`-based test harness for `app/` | medium | `src/app/` has zero `#[test]`s; architecture.md already flags the dialog focus edge-trigger as "invisible to the test suite" |
| A command-replay log (record the `Vec<Command>` stream) for crash diagnostics and, later, macros | medium | `Workspace::execute(Command)` is already the one chokepoint everything flows through |
| An explicit `schema_version` field on the four persisted JSON stores | small | Cheap now, expensive to retrofit once real user data is on disk in an unmarked format |
| Property/generative tests for `PanelState` invariants once `ViewConfig`/`DirIndex` land | medium | Closes the *class* of bug D10 fixes one instance at a time |

**Scale / performance ideas:**

| Idea | Effort | Note |
| --- | --- | --- |
| Move directory listing (`read_dir`/`jwalk`) off the main thread | medium | Distinct from the deferred "virtualised file list" - this is about the synchronous read, not render cost |
| Cap/pool image-preload thread spawns and gate them by volume speed | small | Complements D16; a slow network volume can turn the look-ahead cache into a thundering herd |
| Make the free-space preflight non-blocking with a timeout | small | Same root cause as D23's `free_space()` hang fix, framed as a proactive UX improvement |
| Make the recursive fs watcher opt-out/shallow on non-local volumes | medium | FSEvents doesn't work reliably over SMB/NFS; `notify` falls back to a polling backend that can hammer a share |
| Surface the already-computed `walk_log` cost as a "slow volume" indicator | small | The measurement exists today and is silently discarded after driving an internal skip decision |
| Roll up (not just truncate) the confirmation-dialog scan past `MAX_FLAT_ENTRIES` | small | A 100k-file tree currently just shows "... (truncated)" with no size/count summary of what was cut |

## Tracking

**Round-4 audit + ideas pass: done.** 8 more area-scoped reviewers (the rest
of `workspace.rs`, a fresh `panel.rs` pass, undo/redo completeness,
keybinding/command consistency, filesystem edge cases, a test-coverage-gap
hunt, and the `app/mod.rs`/`render.rs`/`file_list.rs` rendering layer) plus 5
idea-generation angles ran in the same pass. Every defect finding was
adversarially re-verified (two were live-reproduced with a temporary test,
written/run/reverted); zero were refuted. `audit.md`/`architecture.md`/
`recommendation.md` were refreshed to match (README still needed no changes).
See `audit.md`'s round-4 section for the full defect list and this doc's new
**Track E** for the ideas backlog.

**Round-3 audit sync: done.** 10 area-scoped reviewers (security/FFI,
persistence, queue engine, sync/compare/dedup, small utilities, app dialogs,
fresh passes on both god objects, image/media, docs drift) ran; superseded by
round 4 above, but the docs-hygiene fixes from that pass are still in effect.

Done: **C** (docs, round-1 and round-3 batches), and **D1 + D2 + D3**
(commit `177e67c`): save writers return their success bool and the
explicit-save dialogs toast on failure; undo/redo surface a refused rename
instead of swallowing it; the image cache is flushed on directory change.

Next suggested order: **D12 (AppleScript injection) first** - still an actual
local code-execution vulnerability, not a robustness nit, and the fix is small
(escape the interpolated string or drop the AppleScript call). Then the other
round-4 discoveries that are silent-wrong-destructive-action bugs reachable
through completely ordinary interaction - **D19 (dialog retargeting), D21
(conflict-resolution deadlock), D20 (undo coverage), D22 (drag-and-drop
rewrite)** - since these are worse in kind than a crash (they do the wrong
thing to the wrong files with no warning), even though several individually
rank below D6's panics on raw severity. Then **D6 (panics/overflow) + D10
(cursor invariants) + D14 (textdiff DoS) + D15 (unconfirmed drag/toolbar
ops)**. Then **B1 (regex) -> A2 -> A3 -> B2 -> A4 ...**, with D4 (cheap perf),
D11 (id_salt), D18, and D24 (small UI/no-op fixes) slotted in as low-risk
fillers. Schedule **D9** (destructive-op partial-failure integrity, with
on-disk undo tests), **D13** (directory-blind comparison), and **D23**
(filesystem edge cases) as dedicated passes, **D16** (image pipeline) and
**D17** (persistence hardening) whenever those modules are next touched, and
**D5** alongside the `DirIndex` extraction. D10 is also the strongest concrete
motivation for the `ViewState` encapsulation in Track A, and D19's dialog-
snapshot fix is the strongest concrete motivation for the `UiState`
extraction (A5).

Track E (ideas) is deliberately unscheduled - revisit it after the D-track
correctness work above lands, and pull specific ideas into Track A/B once
prioritised.
