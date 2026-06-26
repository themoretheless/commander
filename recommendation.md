# Recommendations

The prioritised plan of what to do next, kept in sync with
[architecture.md](architecture.md) (the target shape) and
[README.md](README.md) (user-facing capabilities).

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

## Tracking

Suggested immediate order: **C (docs) -> B1 (regex) -> A2 -> A3 -> B2 -> A4 ...**
Front-load the trivial doc fixes and the cheap high-value feature, then proceed
down Track A, slotting B2 in right after `ViewConfig` lands.
