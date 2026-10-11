# Structure plan: architecture, SOLID, DRY

Status: plan, not started. Written 2026-10-11 against `main` + PR #20.

`architecture.md` records the earlier structural passes and their accepted
residuals. This plan covers what is still open after them. It stays within the
rules that document sets (no field shuffles, no mechanical `workspace/mod.rs`
conversion, no new abstraction with a single implementation).

## How the candidates were found

A review workflow read one subsystem per agent and proposed refactors with
`file:line` evidence. It hit a usage limit partway through:

- **Read:** dialogs, frame loop, `panel`, `workspace`, copy path, durability.
  These produced the 24 candidates below.
- **Not read yet:** command catalogue, leaf utilities, platform adapters.
  Phase 0 covers them.
- **Not verified:** none of the 24 has passed the adversarial check yet (is the
  problem real, is the fix safe). Each one is verified before it is
  implemented, and dropped if it fails.

## Rules for every step

1. One refactor per commit, behavior-preserving, about 400 changed lines or
   fewer.
2. Before pushing: `cargo fmt --check`; clippy with `-D warnings` for the
   Linux, Windows (`x86_64-pc-windows-gnu`) and macOS
   (`aarch64-apple-darwin`, `CARGO_FEATURE_PURE=1`) targets; the full test
   suite; and the four visual-QA scenarios under Xvfb.
3. In the copy, journal and persistence code, durability, no-clobber,
   cancellation and progress accounting must stay identical. Every change
   there names the existing test that pins the behavior. If no test exists,
   one is added first.

## Phase 0: finish discovery

Run the same find, verify and plan pass on the three subsystems not read yet:

- **Command catalogue** (`command.rs`, `ui_request.rs`, omnibar palette
  helpers): adding one command touches about six `match` blocks (label,
  shortcut, keywords, availability, preview, category). Expected outcome: one
  descriptor table per command (OCP).
- **Leaf utilities** (`theme`, `file_color`, `selection_summary`, `reldate`,
  `fuzzy`, `search`, `query`, `content_index`, `image_cache`, `compare`,
  `sync`, `dedup`): duplicated size formatting, color math and string
  matching.
- **Platform adapters** (`native_effect`, `native_menu`, `desktop_portable`,
  `ports`, `main.rs`, `volume_profile`, `mount_guard`): keep `cfg` gating in
  one place; the composition root should choose adapters per OS.

## Phase 1: low-risk leaf extractions

| # | Change | Principle | Est. lines | Main files |
| --- | --- | --- | --- | --- |
| 1 | One no-replace rename owner: fold `native_copy{,_portable}::rename_noreplace` into `fs_at`'s per-OS exclusive rename | DRY | 280 | `native_copy*.rs`, `fs_at.rs` |
| 2 | One symlink-recreation primitive in `fs_util`. Today there are three copies, with different Windows behavior. | DRY | 80 | `fs_util.rs`, `transfer/buffered.rs`, `native_copy_portable.rs` |
| 3 | Shared per-file progress start/cancel helpers for the five copy primitives | DRY | 110 | `transfer.rs`, `transfer/buffered.rs` |
| 4 | One temp-file commit pipeline, lock scope and flock primitive in `persistence` | DRY | 170 | `persistence.rs` |
| 5 | One health-reporting policy for store save/load outcomes | DRY | 110 | `persistence.rs`, `session.rs` |
| 6 | One fail-closed load/commit mapping shared by the journal and content index | DRY | 70 | `operation_journal.rs`, `content_index.rs` |
| 7 | One first-wins latch for `Workspace.safe_state` (three hand-rolled copies today) | DRY | 90 | `workspace.rs`, `workspace/transfer_queue.rs` |

## Phase 2: UI shell (`app/`)

| # | Change | Principle | Est. lines |
| --- | --- | --- | --- |
| 8 | One `dialog_chrome` builder for the repeated `egui::Window` framing (anchor, frame, title, escape) across about 15 dialogs | DRY | 260 |
| 9 | Button roles (`Primary`, `Secondary`, `Danger`) instead of about 32 hand-styled buttons | DRY | 350 |
| 10 | Render both panes through one per-side function with a read-only `PaneView`. Today left and right are about 120 duplicated lines. | DRY | 230 |
| 11 | One feedback facade (`notify`, `notify_undoable`) replacing 33 hand-built `Toast::new` pushes | DRY | 260 |
| 12 | One drag-session resolver shared by the drop overlay, pane targets, autoscroll and drop execution | DRY | 150 |
| 13 | `ContextMenuPort` decides native vs in-app menu, which removes `cfg!(target_os)` from the frame loop | DIP | 70 |
| 14 | Finish the one-shot focus migration (drop the per-dialog `focused: bool` flags) | DRY | 80 |
| 15 | One virtualized flat-list renderer in the confirm dialog | DRY | 400 |

Item 8 changes how dialogs look, so it needs the visual-QA gate. Item 10 is
where the 36 px pane-height bug fixed in PR #20 was hiding.

## Phase 3: core facades

| # | Change | Principle | Est. lines |
| --- | --- | --- | --- |
| 16 | Move the process-global dir-size cache out of `PanelState` into `panel/dir_size_cache.rs` behind a narrow API | SRP | ~1350 (mostly moved; split into 2–3 commits) |
| 17 | Two filtered-view iterators and one selection-update helper replacing 15 hand-rolled loops in `PanelState` | DRY | 240 |
| 18 | Named listing-transition steps in `PanelState` (`preserving_focus`, `settle_snapshot`, …) | DRY | 130 |
| 19 | `panel/facet.rs` with a single age-deadline rule shared by the predicate and cache invalidation | DRY | 220 |
| 20 | One constructor for staging a `PendingTransfer` and re-running its preflight | DRY | 200 |
| 21 | One `TransferSpec` builder for workspace-queued transfers (four 30-line literals today) | DRY | 150 |
| 22 | Derive both `CommandContext` builders from one base | DRY | 90 |

## Phase 4: durability core (highest care)

| # | Change | Principle | Est. lines |
| --- | --- | --- | --- |
| 23 | `ResumeCheckpoint` owns its layout, size rule, content proof and construction (four call sites today) | DRY | 190 |
| 24 | Operation journal: one terminal-completion writer, one overwrite-backup release, typed record and step lookups | DRY | 230 |

Both items touch crash-recovery code. Before each one, the journal fault suite
in `operation_journal/tests.rs` and the executor tests must be green on macOS
CI, as well as locally.

## Phase 5: system design (product-level)

These follow `ux.md` §7. Each one also removes structure: windows, `UiModal`
variants and request plumbing.

1. **Activity strip.** Merge `transfer_dialog`, `queue_dialog`,
   `history_dialog`, `recovery_dialog` and `safe_state_dialog` into one
   bottom-strip component over the existing transfer queue and receipts.
   This is the largest reduction in modal surface.
2. **Single cross-folder selection.** Retire the `marks`, `shelf` and stash
   UI. The selection algebra stays reachable through omnibar commands.
3. **Panel modes.** Make treemap, diff, sync, duplicates and archive modes of
   a panel instead of windows.
4. **Theme tokens.** Port the warm-graphite palette and the
   color-blind-safe extension colors into `theme.rs` and `file_color.rs`.
   Tokens come from one table, and the test asserts both contrast and
   color-blind separation.

## Order and size

Phase 0 runs first. Phases 1 and 2 can then run in parallel, since they touch
different files. Phase 3 comes after phase 1, and phase 4 comes last of the
refactors. Phase 5 can start any time after phase 2's items 8 and 11, because
the Activity strip builds on the dialog chrome and the feedback facade.

In total that is 24 commits of roughly 100–400 lines each, plus about 10 from
phase 0 and the phase 5 features. None of it changes behavior until phase 5.
