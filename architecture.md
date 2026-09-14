# Architecture

Commander is a dual-pane macOS file manager written in Rust with
[egui](https://github.com/emilk/egui). This document describes how the code is
organised today, the structural debt that has accumulated, and the target shape
the refactoring is moving toward. It is kept in sync with [README.md](README.md)
(user-facing capabilities), [recommendation.md](recommendation.md) (the
prioritised plan plus the requested compact Top-500 review backlog),
[audit.md](audit.md) (the ranked, adversarially-verified list of concrete
defects), and [backlog.md](backlog.md) (the wider 621-item raw inventory: 92
bugs, 243 design/quality problems, 148 improvements, 138 module-scoped
suggestions).

## Guiding principle

> The file-manager logic lives in a UI-independent, unit-tested core; the `app`
> module is a thin egui layer over it.

That split is real and worth protecting: more than 60 focused modules and 881
tests sit under a thin presentation layer. The broad suite passes 878; three
manual/performance harnesses are intentionally ignored there, and the
single-threaded performance timing gate is run separately. The largest debt is
still concentrated in the `panel`, `workspace`, and `transfer` facades, but
their mutable state is now split behind owned controllers and core-to-UI
signalling is a typed FIFO boundary rather than a field-level flag bus.

### External-research constraints (2026-07-14, revalidated 2026-07-18)

The comparative pass in [research.md](research.md) adds five constraints to the
target architecture without changing the current migration order:

1. Paths remain the primary visible model. Search providers, facets, tags, and
   virtual collections augment `PanelState`; they do not replace navigation.
2. Every listing/search/preview/hash job has a generation, cancellation path,
   priority, and resource budget. Results from an obsolete snapshot cannot
   mutate current panel state.
3. `TransferSpec` evolves toward a serializable operation state machine. A
   low-level success is not a committed operation until final placement and
   end-to-end policy checks succeed.
4. Recovery is one bounded context spanning undo eligibility, checkpoints,
   conflict versions, journal replay, and uncertain-failure lockout. Dialogs
   render that policy; they do not invent it.
5. The UI preserves stable geometry and separate active-pane, focus, cursor,
   selection, and mark states. Pointer-only actions have keyboard equivalents.

These constraints reinforce, rather than replace, the shipped `ViewConfig`,
`TransferQueueController`, `UiState`, journal proof model, and injected native
effect ports. `UndoCenter` now owns the in-memory history timeline and
identity-bound replay reservations. The shared persistence boundary and
versioned envelope are now shipped too. Asynchronous listing publication and
the placement→`mark_completed` crash windows (overwrite and non-overwrite) are
on `main`. The remaining structural work is the open facade/integrity PR stack
plus three accepted residuals: a descriptor-relative filesystem effect port, a
streaming tree planner, and cross-process CAS / Persist-envelope migration for
the operation journal and content index. See
[Structural Ideal Definition of Done](#structural-ideal-definition-of-done).
The typed queue portion is shipped as `ui_request::UiRequestQueue`; historical
roadmap references to an Effect bus describe that completed migration. `G044`
has an owner in `volume_profile`; `path_identity` supplies the core of `G057`.
`ports`/`provider_runtime`, `workload`, and the journal transition machines own
`G081-G090`. `measurement`, `benchmark_fixture`,
`capability_diagnostic`, `support_bundle`, `feature_flags`, and `klm` own
`G091-G100` without adding policy to `Workspace`.

The fixed 100-repository cohort was refreshed in full on 2026-07-18: every
entry remained reachable and non-archived. That pass produced the smaller
`H001-H012` hardening ledger in `research.md`; the subsequent `I001-I010`
follow-up closes preview, command, watcher, naming, capability, and retention
gaps. `J001-J010` is the new unimplemented idea set. None changes the
longer-term module migration order below.

## Module map (current)

### Core (UI-independent, unit-tested)

Grouped by the bounded context each module really belongs to:

- **Navigation / panel state**: `PanelState` is the public coordination facade
  over `panel::listing` (rows, status, checked revision, filter cache),
  `panel::view` (private-field `ViewConfig`, filters, bounded per-folder
  memory), `panel::sort`, `panel::selection`, `panel::watcher`, and
  `panel::size_index`. `watcher_policy`, `watcher_health`, `jumplist`,
  `crumbs`, `scan`, `collections`, and `tree_overview` own adjacent policy.
- **Discovery / search**: `query` is the canonical grammar, `search` owns
  cancellable generations and provider composition, `content_index` owns the
  optional root-scoped snapshot, and `archive` provides bounded ZIP browsing
  and member search. `fuzzy`, `image_cache`, and `io_budget` are shared
  mechanisms, not UI policies.
- **Workspace / coordination**: `workspace` composes two panels, pending
  operations, compare/sync/drop glue, and typed controllers:
  `workspace::transfer_queue` owns queue admission, active-worker identity,
  sequencing, cancellation, safe-state publication and retirement;
  `workspace::delete` and `workspace::space_probe` own their asynchronous
  lifecycles. `undo::UndoCenter` exclusively owns stack transitions,
  revisions, and replay reservation state while `Workspace` executes the
  filesystem action and applies typed settlement outcomes. The workspace
  integration tests live in `workspace/tests.rs`. `ui_request` is the
  toolkit-independent FIFO intent boundary; `command` owns the `Command` enum
  + key mapping and typed composable predicates over pure `CommandContext`
  snapshots used by every action surface).
- **Selection / comparison**: `compare` (cross-pane classification + selection
  set logic, extracted from `workspace`), `selset`, `selection_summary`,
  `dedup`, `textdiff`.
- **Operation contract / recovery**: `operation` owns IDs, durability, and
  failure classes; `operation_journal` owns serializable event transitions and
  recovery (production in `operation_journal.rs`, tests in
  `operation_journal/tests.rs`); `path_identity`, `filesystem_policy`,
  `mount_guard`, `version_store`, `undo`, and `sync_guard` supply identity
  proof, filesystem capability policy, remount safety, bounded version
  retention, reversible history, and circuit breakers.
- **Transfer execution**: `transfer::executor::TransferExecutor` exclusively
  coordinates preflight, conflict review, identity and mount fences, journal
  transitions, placement, source cleanup, rollback, and terminal publication.
  `transfer::backend` selects replaceable object-safe native/clone, delta,
  sparse, and buffered staging ports; their typed receipt carries artifact
  identity, byte/fast-path accounting, and durability proof but no final
  namespace capability. `native_copy`, `delta_copy`, and `verified_hash` own
  specialized data paths; `volume_profile` and `transfer_tuning` own
  capability/telemetry policy; `opqueue` is surfaced through the queue panel.
  `conflict`, `fs_util`, `rename`, `rename_order`, `sync`, and `shelf` remain
  adjacent operation helpers.
- **Capability / workload boundaries**: `ports` defines narrow preview,
  search, filesystem, hashing, clipboard, opener, Trash, free-space, and
  main-thread context-menu contracts. `native_effect` contains the real macOS
  adapters; native-menu callbacks produce invocation-bound typed intents and
  defer side effects until AppKit tracking ends. `provider_runtime` enforces lazy
  capability activation, startup budgets, and out-of-process optional
  providers; `workload` owns priority, quotas, cancellation, backpressure,
  immutable snapshots, generation rejection, and scheduler telemetry;
  `feature_flags` owns persisted rollout cohorts and atomic runtime kill
  switches for optional providers.
- **Measurement / diagnostics**: `measurement` owns latency distributions,
  startup phases, and versioned CI budgets; `benchmark_fixture` generates
  deterministic empirical trees; `capability_diagnostic` explains per-volume
  fast paths and fallbacks; `support_bundle` exports capped, salted-redacted
  evidence including watcher health; `klm` checks the ten core operator
  workflows.
- **Presentation-independent helpers**: `listing_export`, `reldate`,
  `file_color`, `clipboard`, `cmdtemplate`, `bookmarks`, `smart_folder`,
  `session`, `persistence` (shared item-level recovery and path-free health),
  `density`, `display_name` (Unicode/extension-aware compact names),
  `focus_mode`, `quick_actions`, `treemap`, `toasts` (a pure,
  time-driven toast queue with an injected clock; no egui types),
  `lock_util` (the single poison-recovery policy at worker/UI mutex borders).

### UI adapter (`app/`, egui)

`app/mod.rs` owns the `App` presentation shell (theme, zoom, image cache, tree
widget, and operation surfaces). `app::ui_state::UiState` owns transient input,
modal buffers, modal FIFO state, and Escape ownership. Per-frame orchestration
lives in `app/update.rs`, including the single `UiRequest` snapshot drain;
input translation lives in `app/keys.rs`; one file per
dialog/sheet (`confirm_dialog`, `batch_rename_dialog`, `sync_dialog`,
`find_dialog`, `recovery_dialog`, `safe_state_dialog`, `collections_dialog`,
...); row rendering in `app/file_list.rs` and `app/render.rs`; native macOS
menu in `native_menu`. Toolkit-independent accessibility and responsive-layout
contracts live in `accessibility`; operation presentation vocabulary lives in
`operation_view` rather than individual dialogs. `app/developer_panel.rs`
renders immutable diagnostics snapshots and sends explicit feature-control or
export commands; it does not own measurement, persistence, watcher, or rollout
policy.

### Size hot-spots

Line counts are approximate on current `main` (2026-09-14). Ideal coordination
facades are ~600–800 lines once byte-path/tests and coherent command modules
are extracted; the open PR stack below is the path there, not another field
shuffle.

| File | Lines | Note |
| --- | --- | --- |
| `src/transfer.rs` | ~3,050 | Public progress/spec facade; byte-copy primitives live under `transfer::{buffered,sparse,parallel_tree}` |
| `src/panel.rs` | 4,553 | Coordination facade; mutable listing/view/selection/watcher/size state is already delegated |
| `src/operation_journal.rs` | ~3,090 | Durable transitions, proof validation, migration, and recovery; production facade only |
| `src/operation_journal/tests.rs` | ~1,545 | Fault/model suite intentionally separated (workspace-style `#[cfg(test)] mod tests;`); CAS and persist-envelope adoption remain follow-ups |
| `src/workspace/tests.rs` | 3,468 | Integration/fault suite intentionally separated from the production facade |
| `src/workspace.rs` | ~2,800 | Two-panel orchestration; fileops facades + queue/delete/space/undo controllers |
| `src/app/update.rs` | 2,118 | Per-frame hub and typed request dispatcher; dialog buffers live in `UiState` |

## Agent/critic remediation sequence (2026-07-21 to 2026-07-28)

Each architectural concern was implemented by a scoped agent and reviewed by
an independent critic. P0-P2 findings were corrected and re-reviewed before a
track was accepted; one unsafe journal patch was rejected rather than merged.

| Track | Result | Remaining boundary |
| --- | --- | --- |
| Workspace decomposition | accepted | `TransferQueueController`, `DeleteController`, `SpaceProbeController`, and `UndoCenter` own their state; `workspace::fileops` owns mkdir/rename/batch-rename/pending-confirm/drop; gather/duplicates and `workspace/mod.rs` assembly remain |
| Transfer executor/backend boundary | accepted after adversarial hardening | `TransferExecutor` owns transactional effects; native/clone, delta, sparse, and buffered ports only stage artifacts and return typed receipts |
| Typed UI request queue + `UiState` | accepted | FIFO/modal/Escape ownership and dialog buffers are centralized; `App` retains presentation-only state |
| Operation journal/recovery proof model | accepted after a fresh redesign | stable path identities, explicit transitions, migration validation, restart/fault/model tests; UI repair decisions remain explicit |
| Panel ownership split | accepted | listing/view/sort/selection/watcher/size owners are separate; the public coordination facade is still large |
| Async text preview | accepted after timeout, identity, and worker-retirement fixes | retain the isolated one-worker executor and 256 KiB text budget |
| Atomic persistence/session save | accepted | callers must keep distinguishing pre-commit failure from committed-not-durable |
| Workload dependency injection | accepted | migrate remaining global-runtime consumers incrementally |
| Dialog/UI UX contracts | accepted | keep modal Escape and opening snapshots centralized |
| Native visual/accessibility QA | accepted after fail-closed hardening | four Glow scenarios are strict CI gates; actual NSMenu and full-rect topology checks are automated; popup pixels, VoiceOver and real mixed-monitor journeys require exact-subject human attestation |
| Native effect ports and context menu | accepted | Clipboard, Trash, opener and free-space are injected; pure menu intents are invocation-bound; a general persistence port remains |

The first journal attempt was intentionally rejected. The later implementation
landed only after stable identity, transition, restart and side-effect fault
proofs were explicit. That sequence remains the safety contract: a large patch
is not progress until its recovery invariants are demonstrated.

### Research milestone 1 (G001-G050)

The first 50 research proposals were implemented as five bounded slices rather
than folded into `Workspace`:

| Slice | Primary owners | Contract |
| --- | --- | --- |
| G001-G010 | `panel`, `command`, `collections`, `tree_overview` | Re-find paths and preserve navigation context |
| G011-G020 | `query`, `search`, `content_index`, `archive` | Stream, cancel, rank, and inspect discovery work |
| G021-G030 | `operation`, `sync_guard`, `transfer` | Fail closed before and around filesystem mutation |
| G031-G040 | `operation_journal`, `version_store`, `undo`, recovery UI | Prove, resume, roll back, and repair durable operations |
| G041-G050 | `volume_profile`, `transfer_tuning`, `delta_copy`, `verified_hash`, `io_budget` | Choose and explain bounded transfer/resource paths |

The placement invariant is now explicit: a data path writes only to a hidden
sibling staging path; source and destination identities are revalidated; the
requested durability check runs; only then may a no-replace rename or swap
make the effect visible. Checkpoints refer to the same staging inode and a
logical boundary. Buffered recovery truncates an uncheckpointed tail; seeded
delta recovery rewrites from its last fixed/FastCDC boundary. Both still pass
whole-file verification before final placement.

### Research milestone 2 (G051-G100 complete)

The second milestone landed in ten-item slices with policy kept outside
the egui adapter:

| Slice | Primary owners | Contract |
| --- | --- | --- |
| G051-G060 | `filesystem_policy`, `mount_guard`, `path_identity`, `operation` | Model filesystem identity/capabilities and fail closed across remount or policy changes |
| G061-G070 | `operation_view`, `opqueue`, Operations Center, transfer UI | Use one phase/progress/failure vocabulary across queue, history, errors, and recovery |
| G071-G080 | `accessibility`, `theme`, file rows, toolbar, Operations Center | Preserve distinct focus/state channels, non-color cues, assistive semantics, reduced motion, high contrast, 200% layout, and non-drag alternatives |
| G081-G083 | `ports`, `provider_runtime`, `search`, `content_index` | Keep optional providers narrow, capability-scoped, startup-budgeted, and outside the UI process |
| G084-G087 | `workload`, `search`, `content_index`, `image_cache`, `transfer` | Admit heavy work through one priority/quota scheduler and reject stale generations deterministically |
| G088-G090 | `operation_journal`, `operation_verification` | Validate every transition and preserve data across injected side-effect failures, crash restarts, and small conflict state spaces |
| G091-G094 | `measurement`, `benchmark_fixture`, CI manifests | Measure real startup/listing/filter/dialog paths, preserve percentile telemetry, and fail versioned budgets on regression |
| G095-G098 | `developer_panel`, `capability_diagnostic`, `support_bundle`, `feature_flags` | Explain runtime costs and capability routes, export bounded redacted evidence, and disable risky providers without redeploying |
| G099-G100 | `klm`, `operation/README.md`, colocated ADRs | Bound operator growth in ten core workflows and keep operation invariants, ownership, and failure policy beside code |

`accessibility::Preferences` reads system high-contrast/reduced-motion settings
once (with explicit environment overrides for tests). `FocusLayout`, the
control catalog, semantic-channel snapshot, row semantics, overlay placement,
and responsive geometry are pure contracts with unit tests. The egui layer
consumes those decisions: modal surfaces disable background interaction,
toasts avoid the current focus/error rectangles, narrow toolbars use an
overflow menu, and the Operations Center changes from a right panel to a
bottom panel before pane geometry becomes constrained. File-pane widths are
always calculated from the `Ui` area remaining after utility panels carve
their space, never from the raw viewport.

The workload runtime admits search, index, preview, and transfer work against
global and per-kind limits. Every task receives an immutable serializable
snapshot plus a cancellation token; root disconnect and newer generations
cancel obsolete work, and stale completion is counted rather than published.
Journal operation/step status changes now pass through typed event matrices.
The test-only `operation_verification` harness performs real typed filesystem
effects, injects faults immediately before and after each one, restarts from
JSON after every durable transition, and exhausts the small
copy/move/sync-conflict space against no-loss invariants.

Performance CI executes the same probes used by the application instead of
checking a checked-in "current" result. The probes cover startup construction,
a 512-entry first listing, filter response, and operation-dialog model/export
work against `ci/performance-budgets.json`; telemetry retains bounded
p50/p95/p99 distributions and real monotonic cancellation samples. Optional
provider reads use an atomic flag mask on hot paths, while configuration and
support exports take one coherent snapshot. Operation decisions are indexed in
[`src/operation/README.md`](src/operation/README.md) with three accepted ADRs
for durable invariants, state ownership, and failure/recovery policy.

## Comparative hardening slice (2026-07-18)

The `H001-H012` pass follows one rule: policy is computed in a pure core owner,
mutable runtime state stays with the subsystem that observes it, and egui only
renders the resulting state or sends an explicit command.

| Concern | Policy owner | Runtime owner | Presentation boundary |
| --- | --- | --- | --- |
| Cross-pane comparison | `compare`, `conflict`, `sync` | panel/workspace snapshots | compare labels, colors, conflict sheet |
| Action availability | typed `command::CommandPredicate` composition | immutable `Workspace` context snapshots | palette, toolbar, keyboard feedback, contextual key bar |
| Config recovery | `persistence` | each typed store | one toast plus developer counters |
| Image preview | `image_cache` target/provider/admission state | bounded cache/decoder workers and result map | loading, failed, retry, close, provider-health states |
| Directory watching | `watcher_policy` plus `watcher_health` facts | `PanelState` native/polling lifecycle and event generations | developer panel and support bundle |
| Compact names | `display_name` Unicode/suffix policy | immutable row data | file row plus full-name tooltip/accessibility label |
| Volume action gating | `filesystem_policy::CapabilityMatrix` | five-second workspace pane cache | every command surface's enabled state/reason |
| Version retention | `operation::VersionRetentionPolicy` | `version_store` manifest and immutable `TransferSpec` | transfer, delete, and sync review surfaces |

The important flows are intentionally short:

1. An action surface requests one immutable context snapshot (complete for the
   palette/keyboard, minimal for always-visible controls), evaluates the
   command's typed predicates, and renders, explains, or invokes that answer.
2. A config store parses its top-level JSON, recovers each valid item through
   `persistence`, and reports aggregate counts without recording values/paths.
3. Preview requests capture a physical viewport target, enter `Loading`, and
   acquire one of four decoder slots. ImageIO/video/standard providers decode
   toward that target behind a hard timeout and publish either a texture or a
   stable classified failure. Retry explicitly invalidates the failure.
4. Direct watcher events increment a generation and publish once per bounded
   batch. Overflow/backend errors bypass batching, invalidate the incremental
   view, back off, reconnect, and force a full reconciliation.
5. A versioned mutation commits the new version record first, atomically
   publishes the retained manifest, and only then removes expired stored data.

### Hard invariants

1. A directory pair is never classified content-identical from byte size and
   modification time alone.
2. File/folder type conflicts never inherit a conditional overwrite policy.
3. One malformed persisted item cannot erase valid sibling records.
4. Every visible unavailable palette/toolbar action has one core reason and
   cannot execute through that surface.
5. The egui frame never opens or decodes an image.
6. No accepted decoded pixel/color buffer exceeds 256 MiB or a 32,768-pixel
   dimension; allocation failure is data, not a panic.
7. A failed preview remains failed until explicit retry or context invalidation,
   so the UI cannot oscillate into an endless spinner.
8. A watcher that failed to create or subscribe is not counted as active.
9. A watcher gap/reconnect requires full reconciliation, and exported watcher
   diagnostics contain counters rather than paths.
10. One watcher burst publishes at most one ready generation per coalescing
    window; a remote/unknown root uses shallow polling rather than an unbounded
    recursive native assumption.
11. Compact file-row text preserves a meaningful regular/compound extension;
    assistive semantics retain the unshortened name.
12. Copy requires a writable destination, Move requires writable source and
    destination, and local mutations consume the shared volume matrix.
13. Retention always keeps the newest verified version for every original
    path, even when the age window has elapsed.
14. A failed retention-manifest write deletes no previously recorded version.

### Greenfield difference

If this application were starting today, `PanelState` would not own watcher
construction, backoff, reconciliation, listing, cursor, and selection in one
type. A `DirectoryObserver` state machine would emit typed snapshots into a
panel reducer. Command predicates would live behind a small `ActionPolicy`
interface from day one, persisted stores would share one versioned envelope,
and preview/search/index work would enter the same bounded scheduler instead
of acquiring separate lifecycle adapters over time. The current pass moves in
that direction without forcing a high-risk rewrite of the working core.

## The core <-> UI boundary today

Three mechanisms connect the core to the shell:

1. **Command dispatch (clean).** `app/keys.rs` translates egui events into
   toolkit-independent `KeyPress`es, `command::map_keys` maps them to
   `Command`s, and `Workspace::execute(Command)` runs them. This direction is
   healthy.

2. **Typed UI requests (clean, with a deliberate vocabulary dependency).**
   `Workspace` owns one private `UiRequestQueue`; producers emit payload-bearing
   `UiRequest` values and `app/update.rs` drains one fixed FIFO snapshot per
   frame. Requests created during dispatch wait for the next frame, deferred
   modals retain order, and the first pending modal owns focus/Escape until it
   opens. `Rename`, `Archive`, and safe-state recovery carry immutable path or
   operation identity payloads. The core knows the intent catalogue, but not
   egui types, dialog buffers, validation, or rendering.

3. **Typed native effects (clean at the OS edge).** The shell owns
   main-thread-only context-menu, clipboard and opener ports; `Workspace` owns
   thread-safe Trash and free-space ports. Native callbacks return typed
   selections/outcomes, and the app reduces those outcomes only after the
   native tracking call returns. The remaining leak is smaller: a few
   navigation/drag fields are still exposed by the `PanelState` facade even
   though listing, view, selection, watcher and size invariants are private.

## Known structural debt

- **Two oversized coordination facades remain.** `Workspace` still coordinates
  panels, pending operations, history action execution, compare/sync, and
  gather/duplicates glue, but queue, delete, free-space, and undo timeline
  state live in owned controllers, and coherent mkdir/rename/batch-rename/
  pending-confirm/drop commands now live under `workspace::fileops` with thin
  `Workspace` wrappers. Its tests remain out of the production file.
  `PanelState` still exposes a broad method surface, but
  listing/revision/filter cache, view config/memory, selection, watcher, and
  size index are separate owners. The next useful reductions are gather/
  duplicates facades and a later `workspace/mod.rs` assembly, not another
  state-field shuffle.
- **Transfer ownership is split; byte-copy primitives are module-local.**
  `TransferExecutor` is the single transactional coordinator and
  `transfer::backend` is the staging-only port boundary. Buffered,
  sparse, and parallel tree-copy primitives now live in
  `transfer::{buffered,sparse,parallel_tree}` beside that boundary; the
  public `TransferSpec` / progress API is unchanged. Overwrite and
  non-overwrite resume now observe a proven placement that crashed before
  `mark_completed` and write the terminal effect proof instead of failing
  closed. Remaining residual work is a descriptor-relative filesystem
  effect port and a streaming tree planner that does not materialize every
  file before parallel copy.
- **The request catalogue remains shared vocabulary.** The old 25-field flag
  bus is gone, but adding a new shell intent still adds one `UiRequest` variant
  and one dispatcher arm. Keep payload and ordering policy in `ui_request` and
  presentation state in `app`; do not let the enum grow dialog implementation
  details. This is a standing ideal gate, not an open defect.
- **The primary desktop and persistence boundaries are explicit.** Preview,
  filesystem, hash, context-menu, Clipboard, Trash, opener and free-space calls
  sit behind narrow typed ports/adapters. Native menu callbacks cannot mutate
  files or launch services while AppKit is tracking; invocation and target
  identity are revalidated before a deferred effect. `persistence::Persist`
  owns bounded byte reads and revision-checked atomic commits, while typed
  stores own schema and recovery policy through one versioned envelope.
  Bookmarks/session receive the same injected port through `App`/`Workspace`;
  feature flags and the version manifest use the boundary and fail closed.
  Cross-process CAS, descriptor-relative opens, and migration of the operation
  journal/content index onto the shared persist envelope remain explicit
  follow-ups. Journal production code and its fault/model suite are already
  split the same way as workspace (`operation_journal.rs` +
  `operation_journal/tests.rs`); that split does not change resume/reconcile
  ownership.
- **Supply-chain acceptance is explicit and CI-enforced.** `deny.toml` checks
  the full all-features lockfile for advisories, yanked crates, licenses,
  wildcard requirements, and unapproved registries or Git sources. The current
  result is zero known vulnerabilities and one accepted unmaintained advisory:
  `RUSTSEC-2026-0192` for the Linux Wayland/winit `ttf-parser` path. Its owner
  is `@themoretheless`, review is due on 2026-10-21, and the CI date guard
  hard-expires the waiver at 00:00 UTC on 2026-10-28. The gate verifies the
  SHA-256 of pinned `cargo-deny` version `0.20.2` before execution. Forty-one
  duplicate-crate groups stay at warning severity as explicit dependency debt;
  there are no broad duplicate skips or GPL/LGPL license allowances. Run the
  same gate with the command shown in the README.
- **Go-to-path probing and panel listing are asynchronous on `main`.**
  `pathname` performs only lexical parsing and exact tilde expansion; injected
  `DirectoryProbePort::probe` owns the one filesystem `metadata` call.
  `PathProbeController` captures home and opening-panel context, debounces for
  200 ms, and admits at most two tasks through the dedicated two-worker
  `PathProbe` quota, never more than one for the current binding. Further edits
  collapse into one latest-wins candidate while stale slots retire. Freshness
  is checked against the exact local
  dialog/generation/raw/path binding rather than registering transient dialog
  roots in the scheduler's persistent generation map. Stale A-to-B-to-A
  outcomes only retire their slot; current cancelled, abandoned, panicked and
  disconnected outcomes terminate explicitly. `Go` and Enter consume only an
  exact current `Valid` binding. PR #4 moved `PanelState::navigate_to` /
  refresh listing publication onto `TaskKind::Listing` with generation-checked
  publish, retires prior rows under `DirStatus::Loading` on binding change, and
  treats listing (not the advisory probe) as the navigability source of truth —
  closing the probe-to-listing TOCTOU for the ideal gate.
- **Panel async ownership is split; the facade is still large on `main`.**
  `DirectoryWatcherState` and `SizeIndex` own generation, binding, retry and
  bounded-cache state; `ListingState` owns rows and filter invalidation.
  `PanelState` intentionally coordinates their atomic publication. Further
  shrinks must extract coherent operations (DragState + visit/preview/nav
  already live under `panel/`), not move fields back together.
- **The comparison bounded context is directory-blind.** `sync::compare`,
  `compare::classify_entry`, and `conflict::detect` all classify entries using
  only `FileEntry.size`/`modified`, and every directory's `size` is hardcoded
  to `0`. None of the three checks `is_dir`, so folder-level Sync/Compare/
  Conflict results are resolved on synthetic data (audit round-4 #27, #28).
  One `is_dir` guard, or reusing the existing recursive `dir_size_cache`,
  closes all three call sites at once.
- **Persistence migration is intentionally incremental.** Bookmarks support
  item-level recovery and source quarantine, while session state is strict;
  both share the injected port and envelope without write-on-read. Feature
  flags and the version manifest reject corrupt, unreadable, wrong-store, and
  future-schema input without replacing it. Legacy utility stores such as
  smart folders, command templates and the panel cache still use the atomic
  compatibility facade; the operation journal and content index need their own
  streaming/schema migrations before they can adopt the envelope safely
  (accepted residual alongside cross-process CAS).
- **Dialog buffers are centralized, while visual modality remains an egui
  composition contract.** `UiState` owns every transient modal buffer and the
  FIFO/Escape router; opening contexts capture panel/directory/path identity.
  Dialogs are still rendered as `egui::Window`, so focus trapping, stacking
  and background disabling remain explicit application policy. Headless tests
  cover ownership and the running-app visual gate covers the real frame; native
  VoiceOver navigation remains a release check.
- **`undo::Action` covers Move/Rename/Gather and Delete-to-Trash.** `Move`,
  `BatchRename`, path-stable `Rename`, typed `Gather`/`Ungather`, and
  `Trash`/`RestoreTrash` are undoable. Trash undo restores through
  `version_store` (not Finder put-back): user deletes always run the Versioned
  durability profile so a restore copy exists before the Trash port runs.
  Gather folder cleanup is a transfer-owned post-success action: undo removes
  only an empty folder, reports cleanup failure normally, and redo recreates
  the exact path before moving. Partially failed Gather rolls completed
  placements back out of the operation container and removes the orphan
  folder; cancel/mount-retry paths surface `undo_placement` failures instead
  of dropping them. Undo coverage should become an invariant checked for every
  mutating command.
- **The drag-and-drop bug cluster was closed as one ownership change.**
  `PanelState::begin_drag` now owns selection semantics, both panel renderers
  receive the global drag state so destination rows can advertise targets,
  panel backgrounds explicitly target their current directory, and
  `Workspace::take_drop_plan` treats a missing target as cancellation. Busy
  early returns clear the whole drag session. Those fields live in a
  `DragState` value object under `panel/drag.rs` (PR #12).

## Structural Ideal Definition of Done

Structure is “ideal” for this repo when the gates below hold. Status below is
against `main` after PRs #7–#13 merged (2026-09-14).

| Gate | Status |
| --- | --- |
| Hotspot facades ≤ ~600–800 lines of coordination; byte-path/tests extracted | **Structure done on `main`.** Byte-path (`transfer/{buffered,sparse,parallel_tree}`, PR #10), journal tests split (PR #11), panel op-boundary (`panel/{drag,visit,preview,nav}`, PR #12), and workspace fileops (PR #13) are extracted. Absolute facade line counts still exceed the coordination target; further shrink is ongoing hygiene, not a blocking residual. |
| Mutating commands write undo or a documented non-goal | **Done on `main` for Phase1 residuals.** Move/Rename/BatchRename/Gather covered; Delete-to-Trash undo via `version_store` (PR #7); Gather `undo_placement` surfaced (PR #8); native-copy D23 closed or accepted (PR #9). |
| Listing off the UI thread; probe→listing TOCTOU closed | **Done on `main`** (PR #4). |
| Placement→`mark_completed` crash window closed for overwrite + non-overwrite | **Done on `main`** (PRs #5 and #6). |
| New shell intents do not drag dialog details into `UiRequest` | **Standing policy shipped**; keep presentation in `app`, payloads/ordering in `ui_request`. |
| Docs residuals = only the three accepted items below | **Done.** Only the three accepted residuals remain open. |

### Accepted residuals (only)

1. **Descriptor-relative filesystem effect port** — namespace effects that do not
   re-open by path after a proven binding.
2. **Streaming tree planner** — parallel copy that does not materialize every
   file before planning.
3. **Cross-process CAS / Persist envelope for journal + content-index** — if
   those stores are not already solely on the shared versioned envelope with
   cross-process compare-and-swap commits.

Phase1 integrity and facade-extraction items from PRs #7–#13 are closed on
`main`; they are not additional long-term residuals.

## 2026-07-09 SOLID/DRY reading slices

This pass deliberately does not physically move every module in one huge diff.
Instead, it splits the project into ten small review/refactor chunks that line up
with Track F's 500 findings in [recommendation.md](recommendation.md). Use this
as the human reading order before executing the larger 75-module migration plan
below.

| Chunk | Primary files / concern | How to use it |
| --- | --- | --- |
| 1-50 | Focus mode, preload, temp/test helpers, recent/mask/path dialogs, export/date/theme | Clear small correctness and per-frame UI costs first; most are low-risk leaf fixes. |
| 51-100 | Text diff, saved searches, shelf, key guards, rename dialog, toasts, diff dialog | Stabilise user-visible command/dialog semantics before touching larger state owners. |
| 101-150 | Treemap/session/crumbs, dedup/clipboard/file-color, undo/fuzzy/scan/query/sync toolbar | Pull pure helpers and persistence boundaries apart; many are test-first refactors. |
| 151-200 | Jump list, compare, duplicates/run-command dialogs, batch rename, tree | Attack cross-pane correctness, saved-command UX, and rename/tree state in narrow commits. |
| 201-250 | Transfer/find/palette/sync/bookmarks/rename | Pair blocking-work fixes with command-surface cleanup; keep every commit runnable. |
| 251-300 | Selection summary/native copy, App state, command templates, rename order, opqueue | Extract durable value objects and queue state before broader orchestration changes. |
| 301-350 | Native menu, image cache, fs utilities | Treat this as the OS-integration and media-safety slice; D12 was fixed here. |
| 351-400 | Render, file list, confirm dialog | Apply the designer pass: interaction affordances, disabled states, and visual feedback. |
| 401-450 | Command map and app/update first half | Reduce frame-loop branching; move command effects toward one typed queue. |
| 451-500 | App/update second half | Finish the per-frame orchestration split after the smaller pure pieces are stable. |

Design notes from the 3-iteration pass:

- **Iteration 1, inventory:** the first 500 findings were pulled from the
  621-item raw backlog without renumbering, preserving code locations.
- **Iteration 2, designer/SOLID pass:** UI chunks are separated from domain
  chunks: rendering and dialog affordances stay apart from transfer, compare,
  rename, persistence, and file-system ports.
- **Iteration 3, review/fix:** the highest-leverage small security fix landed
  immediately (D12 in `native_menu.rs`); the rest of the split stays as small
  commits so a reviewer can understand one bounded context at a time.

### 2026-07-11 three-pass implementation checkpoint

The same inventory -> designer/SOLID -> adversarial-review loop was run again,
this time landing four bounded changes instead of expanding the backlog:

- `SyncAction` owns stable left/right source paths and exposes valid direction
  choices; `SyncState` owns the two directory snapshots. Case-colliding rows
  become an explicit warning/skip state instead of silently selecting the first
  lowercased name.
- `BatchRenameContext` owns the panel, directory, target names, and preview
  collision set captured at open time. The UI renders that directory and the
  core re-reads it only at commit for a current collision check.
- `PanelState::begin_drag` owns which files a row drag means; rendering owns
  hover feedback; `Workspace` owns consuming or cancelling the drop. That
  boundary now lives in the `DragState` value object on `PanelState`.
- `textdiff` owns its complexity budget. Its LCS matrix is one flat allocation
  with checked dimensions, and the dialog handles an over-budget result as a
  normal user-visible state rather than risking process termination.
- `PendingTransfer` now rebuilds its entries, byte budget, conflict names, and
  flat scan after relation-policy resolution. Policy controls remain available
  even when the original plan does not fit, so choosing Skip/Keep can actually
  make the plan executable.
- `Action::Rename` owns inversion of a single path pair. The filesystem helper
  applies no-replace semantics, stages case-only changes, and rolls back a
  failed second rename before the action is recorded.
- `TreemapSnapshot` owns directory identity and sized rows as one value, so the
  title and visualization cannot drift after panel navigation.
- `lock_util::recover` owns mutex-poison policy across transfer, image loading,
  scan, confirmation, and native-copy FFI boundaries. RGBA buffers validate
  both products and reserve fallibly before CoreGraphics receives a pointer.
- `PanelState` owns cursor revalidation after search/facet/sort changes;
  filtered cache reads are bounds-checked and stale cursor access is a typed
  error instead of an empty selection. The O(1) hot `filtered_count` path stays
  unchanged.
- Toolbar availability delegates to queue-aware `Workspace` guards, while
  disabled controls explain the pending/active reason. Copy/Move may still
  queue behind an active transfer; Delete cannot replace in-flight work.
- `Action::Gather`/`Ungather` and `PostTransferAction::RemoveEmptyDir` keep
  filesystem cleanup out of UI code. Move replay validates every source before
  starting, preventing a known half-undo path.

## Target architecture

One organising principle resolves most of the debt:

> A `Command` produces `Effect`s. The core never names a dialog, and
> side-effects go through ports.

```
input -> Command -> Workspace::execute(cmd) -> pushes Effect(s)
Effect = Open(Dialog) | Clipboard{text,label} | Toast(OpOutcome) | ...
app/ drains Effects once, owns every dialog's state, and holds the ports.
```

Target shape:

- **Typed UI request bus (shipped).** The former 25 `*_request`/payload fields
  are one private `VecDeque<UiRequest>`, pushed by both `execute` and App
  adapters and drained once in `app/update.rs`. Dialog `open_*` methods own the
  one-shot focus edge, payload modals retain identity, and modal handoffs are
  tested through the same dispatcher engine production uses.
- **`UiState` (shipped).** Transient input, all dialog buffers, modal ownership
  and Escape routing live in `app::ui_state`; `App` keeps presentation and
  service handles.
- **`ViewConfig` value object (shipped).** Its fields are private, panel
  transitions are the only mutation surface, persisted fields cross one
  `session` bridge, and the initial config is seeded before the first listing.
  Pure comparison lives in `panel::sort`; `ListingState` alone owns row order,
  revision, and the revision-keyed filter cache.
- **Services out of `Workspace` (shipped ownership pass).**
  `TransferQueueController` owns
  queue + active worker + poll/cancel/dismiss/retirement state and returns
  typed outcomes; `compare` is extracted; `UndoCenter` owns history entries,
  timeline revision, and replay reservation state. Replay settlement fails
  closed on stale, foreign, duplicate, or operation-mismatched completions.
  Durable history and path-identity-bound actions still require a versioned
  persistence schema rather than more in-memory controller state.
- **Ports (desktop effects and persistence shipped).** Clipboard, Trash,
  opener, free-space and context-menu capabilities are injected and return
  structured outcomes. The object-safe `Persist` port separates byte I/O from
  typed version/recovery policy; `App` and `Workspace` share one injected
  instance. Compatibility stores remain visible migration work rather than
  hidden alternate ownership.
- **Bounded contexts as modules.** Navigation, Selection, Comparison (done),
  Transfer/ops, View, Persistence each own their types and tests; no core file
  exceeds ~600 lines.

### SOLID/DRY module decomposition (design pass, 3 critique+refine iterations)

The bullets above name the target pieces. The section below is the dated design
map that guided the work; current-status annotations and the module map above
are authoritative when the old line ranges or proposed API differ from shipped
code. In particular, do not reintroduce the superseded `&mut entries_gen`
design or convert `workspace.rs` to `workspace/mod.rs` mechanically. The plan
came out of a dedicated design pass: 3
independent architects drafted a decomposition from different angles (bounded-
context/DDD, strict single-responsibility, minimal-risk-incremental), one pass
synthesized the best of each, then 3 rounds of adversarial critique (SOLID-
compliance, DRY/duplication, and "can a human review this module in isolation"
lenses) each fed a refine step. The plan below is the round-3 output - it
explicitly corrects mistakes its own earlier rounds made (e.g. `Kind`/`kind_of`
originally being split incompletely across two modules; a discarded
`ports::os_integration` trait that turned out to be structurally unusable because
the AppleScript call site is a static `extern "C"` ObjC callback, not a
trait-object call site).

The original target was **75 modules** averaging ~93 lines. Those historical
estimates are sizing evidence, not current line counts. The table below
(grouped by area) is the reviewable summary of each
module's responsibility and dependencies; the responsibility text is trimmed
for the table, and the exact `movesFrom` line ranges in the current files are
given inline in the numbered **migration steps** further down (each step names
the functions/structs it extracts with their `file:line` origin). So the two
lists divide the labour: this table and the reading order are for
*understanding the target shape*, the migration steps are for *executing the
move safely*; they are sequenced by different concerns (comprehension vs.
risk) and are deliberately not a 1:1 mapping (one migration step often
extracts several small leaf modules in one commit).

**Foundational leaves (do first, no god-object dependency)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `pathname` | Parse user-typed paths, validate names, and expose the injected filesystem directory-probe port without canonicalizing lexical input. | ~80 | - |
| `path_probe` | Debounce and exact-binding-check `Cmd+L` directory probes, bound admitted workers and one latest candidate, and publish terminal UI status. | ~815 incl. tests | pathname, workload, workspace::ActivePanel |
| `pending_op` | Represent a copy/move/delete/shelf-drain awaiting confirmation or completion, and the pure helpers that compute its space/undo shape, with zero Worksp… | ~225 | fs_util (OpClass, SpaceVerdict, space_verdict), undo (Action, wrapped opaquely by the loca… |
| `kind (NEW, split out of selection_summary, replaces the previous panel::entry/kind_of half-move)` | The Kind enum (Folder/Image/Video/Audio/Document/Code/Archive/Other) and the kind_of(&FileEntry) -> Kind classifier, as a standalone leaf with no othe… | ~65 | panel::entry (FileEntry, is_static_image, is_video, extension -- kind_of's own inputs) |

**`panel/` split (was `panel.rs`, the `PanelState` god object)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `panel::dir_status` | Classify a directory read outcome (Listed/Empty/Denied/Gone) for empty-state messaging. One pure function plus its enum. | ~30 | - |
| `panel::entry` | The FileEntry value type and its own display/classification methods (from_meta, is_image/is_static_image/is_video, icon, size_display, modified_displa… | ~175 | - |
| `panel::preview` | Build the two kinds of side-panel content (text/image preview, Get-Info card) from an entry plus already-computed dir size/children. | ~90 | panel::entry |
| `panel::mask` | Parse and match the select-by-mask mini-language (comma-separated glob/extension terms with optional subtraction). | ~65 | - |
| `panel::size_display` | Resolve the byte count to show for an entry, folding in a directory's cached recursive size. | ~20 | panel::entry, panel::size_cache (size_of accessor) |
| `panel::natural_sort` | The natural (numeric-aware) string comparison used by every sort column. | ~55 | - |
| `panel::facet` | The quick-filter facet vocabulary (KindFacet, FacetSet) and the one pure predicate that tests an entry against a FacetSet. | ~100 | panel::entry, kind (KindFacet maps onto Kind's variants) |
| `panel::overview` | One-pass folder aggregate (total bytes, largest, oldest) for the status bar. Pure read-only fold over entries plus the size map. | ~55 | panel::entry, panel::size_cache (folder_overview locks self.dir_sizes directly at panel.rs… |
| `panel::persist_cache` | The on-disk dir-size cache: load/save/invalidate. | ~130 | fs_util (write_atomic), ports::persist (Step 28, load_lenient -- load_cache_from_disk migr… |
| `panel::walk_log` | Track when each directory was last walked and how expensive it was, purely to gate re-walking. | ~45 | - |
| `panel::recent_dirs` | Session-wide most-recently-visited directory list for Cmd+P, distinct from one panel's own back/forward history. | ~45 | - |
| `panel::sort` | The sort-column vocabulary and comparator-building function. Single reason to change: sort rule definitions. | ~90 | panel::natural_sort, kind (kind_of, used by SortColumn::Kind) |
| `panel::filter_cache` | The generation-keyed cache of filtered indices and its ensure/read accessors. | ~120 | panel::facet |
| `panel::view (ViewConfig value object, Track A3)` | Bundle sort_col/sort_order/folders_first/natural_name_sort/show_hidden into one value object with sort_entries-driving methods (toggle_folders_first,… | ~120 | panel::sort |
| `panel::drag_state` | The drag_entries/drop_target fields, now genuinely private (not just pub(crate)) behind a method API: take_drag() -> Option<(Vec<PathBuf>, Option<Path… | ~45 | - |
| `panel::size_cache` | Own the async recursive dir-size/dir-count computation: the Arc<Mutex<HashMap>> dir_sizes/dir_counts, AtomicBool needs_refresh/sizes_dirty flags, last… | ~190 | panel::persist_cache, panel::walk_log, fs_util |
| `panel::watcher` | Own the fs watcher subscription lifecycle: the watcher/watched_path fields, start_watcher, and the watcher-driven half of poll_fs_changes that decides… | ~100 | panel::size_cache (RefreshHandle/DirtyHandle accessors), panel::persist_cache (invalidate_… |
| `panel::listing` | Read a directory into FileEntry rows and reconcile with the previous cursor/selection. | ~80 | panel::entry, panel::dir_status, panel::view (ViewConfig value object, Track A3) -- reload… |
| `panel::selection_ops` | Single-panel selection mutation: toggle/select-all/invert/extend/select_cursor, the smart-select family (select_junk/select_largest/select_same_extens… | ~260 | panel::filter_cache, panel::mask, panel::size_cache (total_size_selected's size_of lookup) |
| `panel::state (src/panel/mod.rs, formerly panel.rs)` | The trimmed PanelState struct (current_path, entries, view: ViewConfig, drag: DragState, size_cache: SizeCache, watcher: DirWatcher, cursor, selected,… | ~235 | panel::view (ViewConfig value object, Track A3), panel::drag_state, panel::size_cache, pan… |

**Ports & the Effect bus (Track A4/A7 - the hexagon)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `ports::notify` | Define a NotifyPort trait (a single on_progress(&self)-style method) wrapping the UI's repaint-request closure, injected the same way opener already i… | ~55 | - |
| `effects (Effect bus, Track A4 keystone)` | Define the typed Effect enum (Open(DialogKind, just_opened: bool, payload), Clipboard{text,label}, Toast(OpOutcome), RunUndo, RunRedo, ...) that repla… | ~205 | pending_op, app::ui_state::dialog_kind |
| `ports::trash (Track A7)` | Define the Trash trait (delete(&Path) -> Result<(), Error>) wrapping the trash crate, injected into Workspace like opener already is. | ~60 | - |
| `applescript_escape (REPLACES ports::os_integration -- fixes the structurally-unusable-port defect the critique found)` | A plain pure function, escape_for_applescript_literal(s: &str) -> String, that escapes backslashes and double-quotes before a path is interpolated int… | ~15 | - |
| `ports::clipboard (Track A7)` | Define the Clipboard trait (copy_text) and inject it the same way opener is injected, so the effects-bus clipboard drain calls a port instead of ctx.c… | ~50 | clipboard (existing pure formatting module, unchanged) |
| `ports::persist (Track A7, WIDENED vs round 2)` | Define THREE shared persistence helpers instead of one, matching the call sites' actual shapes rather than forcing one signature onto all of them: loa… | ~110 | fs_util (write_atomic) |

**Transfer/undo services (Track A6)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `transfer_center (Track A6, narrowed scope, ISP-fixed)` | Own the transfer queue + pump + poll + cancel + dismiss lifecycle, matching architecture.md's own definition, PLUS the post-transfer panel refresh tha… | ~230 | pending_op, transfer, opqueue, ports::notify, effects, panel::state::Refreshable (a narrow… |
| `workspace::transfer_requests (new, split out of transfer_center)` | Compute a Copy/Move transfer plan from the current selection and target, and resolve conflicts: request_copy/request_move/request_transfer/pending_con… | ~90 | pending_op, scan, conflict, transfer_center (Track A6, narrowed scope), panel::state |
| `undo_center (Track A6)` | Own the undo/redo stack lifecycle: the UndoStack, perform_undo/perform_redo/execute_action (replays a Move or BatchRename inverse), and apply_rename_o… | ~195 | undo, transfer_center (Track A6, narrowed scope) -- poll() returns the Action to push; sta… |

**`fileops/` - one file-mutating operation per module**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `fileops::mkdir` | Create a new folder in the active directory with a collision-free name. One narrow filesystem mutation. | ~25 | fs_util |
| `fileops::delete` | Compute a delete request (gather entries + scan) and execute the Trash calls, counting successes/failures. | ~60 | scan, ports::trash |
| `fileops::pending_op_confirm` | Confirm whichever PendingOp is staged: Delete executes synchronously and reports an outcome; Transfer hands off to transfer_center. | ~40 | fileops::delete, transfer_center (Track A6, narrowed scope) |
| `fileops::rename_single` | Validate and apply one F2 single-file rename, including the same-file/case-only staging logic. | ~90 | pathname, undo_center, fs_util, fileops::batch_rename (active_dir_names, reused rather tha… |
| `fileops::batch_rename` | Compute batch-rename targets/sibling names and apply a RenameRule as one undoable unit, delegating planning to crate::rename and ordering to undo_cent… | ~90 | rename, undo_center |
| `fileops::gather` | Compute the 'gather selection into a new subfolder' plan (collision-free name+path, split as a pure plan_gather_folder helper) and enqueue it as one u… | ~60 | fs_util, transfer_center (Track A6, narrowed scope), undo_center |
| `fileops::duplicates` | Find duplicate files in the active directory: size-prefilter, hash, group, then confirm byte-identity. One pipeline, one reason to change. | ~70 | dedup, fs_util |

**`workspace/` glue (was `workspace.rs`, the `Workspace` god object)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `workspace::shelf_glue` | Drain the shelf into the active directory: build a collision-free plan, split entries into readable/unreadable, start the copy. | ~60 | shelf, transfer_center (Track A6, narrowed scope), ports::notify |
| `workspace::sync_glue` | Translate a computed sync plan (crate::sync::SyncAction list) into two enqueued Copy passes. | ~65 | sync, transfer_center (Track A6, narrowed scope), ports::notify |
| `workspace::drop_glue` | Resolve a completed drag-and-drop into a transfer request: pick source/target via panel::drag_state's take_drag()/set_drop_target() API (never raw fie… | ~95 | transfer_center (Track A6, narrowed scope), panel::drag_state, ports::notify |
| `workspace::selection_glue` | Cross-panel selection commands needing both panels at once: select-same-named, select-by-relation, and the selection-stash algebra. | ~90 | sync (PaneRelation), selset |
| `workspace::bookmarks_glue` | Bind a directory to a quick-jump slot / toggle-favorite -- the one piece of Command handling touching both a panel path and the Bookmarks store togeth… | ~45 | bookmarks |
| `workspace::treemap_query` | Read-only data source for the treemap dialog: aggregate the active directory's entries into (FileEntry, size) pairs for disk-usage visualization. | ~35 | panel::state |
| `workspace::find_query` | The recursive Find walk: run_find alone, as its own leaf module. | ~35 | query |
| `workspace::reveal_nav` | Navigate to a path's parent directory and cursor the revealed entry, used by Find results. | ~30 | panel::state |
| `workspace::diff_query` | Pick the two-file pair (selected-pair or same-named-across-panels) for the diff sheet. One leaf, one caller. | ~40 | panel::state |
| `workspace::preview_glue` | Keep the opposite panel's preview pane in sync with the cursor, and toggle the Get-Info inspector. | ~65 | panel::preview |
| `workspace::mod (src/workspace/mod.rs, was workspace.rs)` | The trimmed Workspace struct (left, right, active, pending_op, transfers: TransferCenter, undo: UndoCenter, effects: EffectQueue, opener, shelf, bookm… | ~260 | effects, command (Command enum execute() matches on), transfer_center (Track A6, narrowed… |

**`app::ui_state` + `App` shell (Track A5)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `app::ui_state::dialog_kind (NEW leaf, resolves the DialogKind ownership defect)` | The DialogKind enum (Rename, Sync, Find, BatchRename, Treemap, RunCommand, Diff, Duplicates, ...) -- one variant per dialog, zero logic beyond derives… | ~20 | - |
| `app::ui_state::dialog_state_types` | The per-dialog editable-state struct definitions and their builder methods (FindState::build_query/from_definition, BatchRenameState::rule, plus RunCo… | ~140 | query, rename, textdiff, dedup, sync, app::ui_state::dialog_kind, effects (for the Effect… |
| `app::ui_state::dialog_buffers (Track A5)` | Group the ~15 per-dialog Option<...State>/buffer fields (renaming, type_ahead, mask_input, path_input, recent_input, palette_input, palette_tick, batc… | ~150 | effects, app::ui_state::dialog_state_types |
| `app::session_bridge` | Convert between App's live UI state and the persisted Session snapshot (to_session, and the session-restore half of App::new). | ~90 | session, theme, panel::view (ViewConfig value object, Track A3) |
| `app::init` | App::new's remaining construction: wire Workspace, theme, image cache, defaults not covered by session_bridge or ui_state. | ~70 | app::session_bridge, app::ui_state::dialog_buffers, workspace::mod (src/workspace/mod.rs,… |
| `app::app_struct` | Hold ws, ui, session-derived toggles (ui_scale, theme_mode, colors, show_tree/show_compare/show_size_bars/focus_mode, tree_* fields), image_cache/toas… | ~60 | workspace::mod (src/workspace/mod.rs, was workspace.rs), app::ui_state::dialog_buffers |
| `app::tree_glue` | tree_expand_to_path and the tree-expansion bookkeeping. Small navigation-adjacent helper, neither session nor dialog state nor core App plumbing. | ~20 | - |
| `app::confirm_outcome_toast` | Turn a confirmed pending-op's DeleteOutcome into a toast message/kind. | ~40 | fileops::pending_op_confirm, toasts |
| `app::focus_mode_glue` | Decide whether focus mode should exit this frame; thin wrapper over the pure crate::focus_mode predicate. | ~20 | focus_mode |
| `app::quick_actions_glue` | Compute the current QuickActionContext and dispatch a chosen QuickAction to the right Workspace call or UI toggle. | ~100 | quick_actions, smart_folder, toasts |

**`app/update.rs` split (Track A4, UI-side half)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `app/update::frame_bootstrap` | Per-frame setup that must run before any dialog draws: repaint heuristics, notify wiring, fs-change polling, drop-target reset (via panel::drag_state'… | ~70 | app::app_struct, panel::drag_state |
| `app/update::effect_drain (Track A4 keystone, UI-side half)` | Drain the Effect queue once per frame: run undo/redo, gather, drain-shelf, cycle-density, and the two clipboard effects, turning each into either a Wo… | ~135 | effects, workspace::mod (src/workspace/mod.rs, was workspace.rs), ports::clipboard, toasts |
| `app/update::dialog_drain` | The eframe::App::update/save impl's ordered list of show_*_dialog calls. Kept tiny and logic-free so reordering dialogs (e.g. | ~40 | app/update::frame_bootstrap, app/update::effect_drain |

**`app/` panel rendering split**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `app/panels::toolbar_panel_render` | Render the top toolbar TopBottomPanel wrapper only (app/toolbar.rs's own body is untouched). | ~15 | - |
| `app/panels::shortcut_bar_render` | Render the bottom shortcut bar: key hints, zoom slider, density/tree/compare/hidden chips, quick-action buttons. | ~155 | density, quick_actions, theme |
| `app/panels::shelf_tray_render` | Render the bottom shelf tray (count/size, removable chips, Clear/Drain buttons) and translate clicks into shelf mutations or a drain effect. | ~100 | shelf, theme |
| `app/panels::selection_hud_render` | Render the floating selection-summary pill. Pure presentation over selection_summary's already-computed data. | ~100 | selection_summary, panel::entry, kind |
| `app/panels::main_area_render` | Lay out the tree sidebar plus the two file panels with the resizable divider, including compare-map cache reuse. | ~165 | compare, app/render.rs (unchanged), app/tree.rs (unchanged), app::tree_glue |
| `app/panels::drag_overlay_render` | Render the floating drag-count/drop-target tooltip that follows the pointer, reading panel::drag_state through its accessor methods. | ~60 | panel::drag_state |
| `app/panels::type_ahead_overlay_render` | Render the fading type-ahead capsule. Tiny, single-purpose. | ~35 | - |
| `app/panels::toast_render` | Render the stacked toast list with countdown bars and inline Undo button, translating a click into an undo effect. | ~80 | toasts, effects |
| `app/panels::drop_input_render` | Detect a mouse-release and forward it to workspace::drop_glue. | ~15 | workspace::drop_glue |

**Optional, beyond committed Track A (do only on explicit demand)**

| Module | Responsibility | ~Lines | Depends on |
| --- | --- | --- | --- |
| `palette (optional, beyond committed Track A)` | Rank and filter the command palette's fuzzy-matched command list, separate from the Command enum and raw key-to-command mapping. | ~370 | - |
| `app/confirm_dialog::flat_list (optional, beyond committed Track A)` | Render the pending-operation's flat file list in the confirm dialog, in both virtualised and animated variants, separate from the dialog's button/tab… | ~220 | scan (FlatList) |
| `app/confirm_dialog::method_tabs (optional, beyond committed Track A)` | **Shipped:** the copy-method tab strip renders from an input value and returns a selection without owning `App`, `Workspace`, or pending-operation state. | ~100 | - |

#### Suggested reading order

For reviewing this plan (or the resulting commits) in small pieces rather than
all at once. Entries are ordered so that, with one noted exception, each builds
only on modules above it - but treat it as a reading aid, not a strict
topological sort: for the exact dependency graph, consult each module's
"Depends on" column in the tables above. The one deliberate forward reference
is `panel::persist_cache` (item 11), which calls a `load_lenient` helper that
only arrives with `ports::persist` (item 51); it is grouped here with the other
panel leaves rather than hoisted 40 positions down, and its single loader call
can be taken on faith until that port is read.

1. `pending_op`
2. `pathname`
3. `panel::entry`
4. `kind (NEW, split out of selection_summary, replaces the previous panel::entry/kind_of half-move)`
5. `panel::dir_status`
6. `panel::preview`
7. `panel::mask`
8. `panel::natural_sort`
9. `panel::sort`
10. `panel::facet`
11. `panel::persist_cache`
12. `panel::walk_log`
13. `panel::recent_dirs`
14. `panel::filter_cache`
15. `panel::view (ViewConfig value object, Track A3)`
16. `panel::drag_state`
17. `panel::size_cache`
18. `panel::size_display`
19. `panel::watcher`
20. `panel::listing`
21. `panel::selection_ops`
22. `panel::overview`
23. `panel::state (src/panel/mod.rs, formerly panel.rs)`
24. `ports::notify`
25. `app::ui_state::dialog_kind (NEW leaf, resolves the DialogKind ownership defect)`
26. `effects (Effect bus, Track A4 keystone)`
27. `transfer_center (Track A6, narrowed scope, ISP-fixed)`
28. `workspace::transfer_requests (new, split out of transfer_center)`
29. `undo_center (Track A6)`
30. `fileops::mkdir`
31. `fileops::delete`
32. `fileops::pending_op_confirm`
33. `fileops::batch_rename`
34. `fileops::rename_single`
35. `fileops::gather`
36. `fileops::duplicates`
37. `workspace::shelf_glue`
38. `workspace::sync_glue`
39. `workspace::drop_glue`
40. `workspace::selection_glue`
41. `workspace::bookmarks_glue`
42. `workspace::treemap_query`
43. `workspace::find_query`
44. `workspace::reveal_nav`
45. `workspace::diff_query`
46. `workspace::preview_glue`
47. `workspace::mod (src/workspace/mod.rs, was workspace.rs)`
48. `ports::trash (Track A7)`
49. `applescript_escape (REPLACES ports::os_integration -- fixes the structurally-unusable-port defect the critique found)`
50. `ports::clipboard (Track A7)`
51. `ports::persist (Track A7, WIDENED vs round 2)`
52. `app::ui_state::dialog_state_types`
53. `app::ui_state::dialog_buffers (Track A5)`
54. `app::session_bridge`
55. `app::init`
56. `app::app_struct`
57. `app::tree_glue`
58. `app::confirm_outcome_toast`
59. `app::focus_mode_glue`
60. `app::quick_actions_glue`
61. `app/update::frame_bootstrap`
62. `app/update::effect_drain (Track A4 keystone, UI-side half)`
63. `app/update::dialog_drain`
64. `app/panels::toolbar_panel_render`
65. `app/panels::shortcut_bar_render`
66. `app/panels::shelf_tray_render`
67. `app/panels::selection_hud_render`
68. `app/panels::main_area_render`
69. `app/panels::drag_overlay_render`
70. `app/panels::type_ahead_overlay_render`
71. `app/panels::toast_render`
72. `app/panels::drop_input_render`
73. `palette (optional, beyond committed Track A)`
74. `app/confirm_dialog::method_tabs (optional, beyond committed Track A)`
75. `app/confirm_dialog::flat_list (optional, beyond committed Track A)`

#### Migration steps (one green commit each, in order)

1. Step 0 (A2, **done 2026-07-28**): the inline test body moved mechanically to `src/workspace/tests.rs` behind `#[cfg(test)] mod tests;`. `src/workspace.rs` deliberately remains a file; converting it to `workspace/mod.rs` would add rename churn with no ownership benefit. The move preserved all `workspace::tests::*` paths and the exact 855-test baseline before later focused tests were added.

2. Step 1 (A2, **done 2026-07-28; async follow-up and lifecycle hardening done 2026-07-28**): `src/pathname.rs` owns pure `parse_dir_input`, `validate_new_name`, typed `DirInputError`/`NewNameError`, and the injected `DirectoryProbePort`. `src/path_probe.rs` owns the debounced worker/controller lifecycle; `AppServices` injects its filesystem and workload adapters. Nineteen focused tests freeze lexical relative/symlink behavior, exact tilde expansion, stable UI copy, universal NUL rejection, debounce/admission bounds, blocking-worker replacement, 100-edit submission bounds, synchronous completion ordering, worker-thread execution, opening-context capture, cancellation/reopen, disconnect/panic retirement, A-to-B-to-A freshness, and same-frame edit-before-Enter ordering. Commit-time live sibling validation and `rename_noreplace` remain unchanged; async panel listing later landed on `main` via PR #4.

3. Step 2: extract pending_op (PendingTransfer+impl, QueuedJob, PendingOp, DeleteOutcome, ShelfDrainOutcome, move_pairs, faithfully_undoable, fit_stats -- workspace.rs:22-230) into src/pending_op.rs. All pure data types/functions, compiler-verified with zero logic change. FIX vs round 2 (ISP smell the critique found, verified: QueuedJob only clones/pattern-matches its stored Action at the poll_transfer call site outside pending_op itself -- pending_op's own logic never branches on which Action variant is stored): change QueuedJob's field from `undo: Option<undo::Action>` to a module-private opaque carrier `undo: Option<UndoPayload>` where `UndoPayload` is a thin newtype wrapping `undo::Action` with no methods of its own beyond construction/unwrap -- this keeps pending_op's own code from needing to know Action's variants while still being honest that the payload IS an undo::Action underneath (a real newtype, not a type-erased Box<dyn Any>, since the consumer -- undo_center -- always knows the concrete type it put in). pending_op's dependsOn keeps `undo (Action, wrapped by UndoPayload, held opaquely by QueuedJob)` but the module doc-comment states explicitly that pending_op never matches on Action's variants, so a future Action variant addition (e.g. D20's Rename) touches undo_center and fileops, never pending_op. Also depends on fs_util (OpClass/SpaceVerdict/space_verdict), transfer (TransferKind/OverwritePolicy/CopyMethod), and scan (FlatList/FileEntry) as round 2 already corrected.

4. Step 3 (WIDENED vs round 2 -- fixes the Kind/kind_of incomplete-split defect the critique found): before touching panel.rs's leaves, extract selection_summary.rs's Kind enum AND kind_of function TOGETHER into a new standalone leaf module src/kind.rs (not panel::entry -- see the module entry below for why). This is its own small commit, landing before panel::entry's extraction (reordered ahead of the rest of Step 3's original leaf list) because panel::entry's own construction needs kind.rs to exist first. Verified in source: Kind/kind_of are consumed by 6 call sites beyond selection_summary.rs itself -- query.rs:6 (`use crate::selection_summary::{Kind, kind_of}`), file_color.rs:6 (`use crate::selection_summary::Kind`), smart_folder.rs:64 (test-only import), app/find_dialog.rs:6 (`use crate::selection_summary::Kind`), app/mod.rs:122 (`FindState.kind: Option<selection_summary::Kind>`), app/treemap_dialog.rs:6 (`use crate::selection_summary::kind_of`), panel.rs:1137-1138 (`crate::selection_summary::kind_of` inside sort_entries), and app/file_list.rs:322 (`crate::selection_summary::kind_of`). ALL EIGHT of these call sites are rewired to `crate::kind::{Kind, kind_of}` in this SAME commit -- this is not deferred to a later step, because a partial rewire (rewiring panel.rs's SortColumn::Kind arm but not query.rs/file_color.rs/find_dialog.rs/app/mod.rs's FindState) is exactly the incomplete-cut failure mode the critique caught. selection_summary.rs itself becomes a pure consumer of crate::kind (its own remaining logic -- the selection-breakdown histogram -- calls kind_of but no longer owns it). Verify kind_of_buckets_by_extension_and_type and every existing test referencing Kind/kind_of still passes unchanged (it is a pure relocation).

5. Step 4 (renumbered, was part of round 2's Step 3): now that kind.rs exists, split panel.rs's remaining pure, dependency-free leaves, each its own small commit (7-9 commits, ~30-140 lines each): panel::dir_status, panel::entry (FileEntry struct/impl + format_size, now calling crate::kind::kind_of for its own Kind-adjacent helpers if any, but NOT re-owning Kind itself), panel::preview, panel::mask, panel::natural_sort, panel::facet, panel::size_display, panel::overview (see this module's own entry below for its corrected dependsOn). ALSO in this step: rewire image_cache.rs's independent is_video_ext (image_cache.rs:219-226, a path-based re-implementation of the exact same extension list as FileEntry::is_video, panel.rs:279-284) to call panel::entry's is_video predicate via a path-taking wrapper, OR -- if image_cache.rs's call site genuinely only has a bare Path and no FileEntry to hand it -- extract the shared extension-list literal itself into a single `pub(crate) const VIDEO_EXTENSIONS: &[&str]` in panel::entry that both FileEntry::is_video and image_cache::is_video_ext match against, closing the drift risk the critique found (a future added extension silently desyncing thumbnail loading from file-listing classification) with a one-array source of truth. Every one of these is a straight cut of already-pure types/functions with its test slice moved alongside -- lowest risk in the plan, do before anything touching PanelState's field layout.

6. Step 5: extract panel::persist_cache (on-disk dir-size cache: CacheEntry, cache_path, dir_size_cache, load_cache_from_disk, invalidate_size_cache, flush_cache), panel::walk_log (walk_log, WALK_COOLDOWN, WALK_EXPENSIVE, reset_walk_log), and panel::recent_dirs (visited_log, VISITED_CAP, push_visit, record_visit, visited_paths, filter_visited) as three separate modules -- independently-changing concerns (on-disk persistence, walk-cost cooldown policy, Cmd+P recent-switcher list) that today sit as loose statics with no dependency on PanelState's own fields. NOTE 1 (carried forward): panel::persist_cache's invalidate_size_cache is called directly from inside the fs-watcher's background callback closure, not just from panel methods -- flag this now so Step 9's watcher extraction accounts for it instead of discovering it late. NOTE 2 (cross-referenced for ports::persist, fixes a round-2 critique gap): panel::persist_cache's load_cache_from_disk (panel.rs:12-52) is structurally a FIFTH instance of the same 'read-file -> serde_json::from_str -> fall back to empty on error' loader pattern that Step 26 (ports::persist) later consolidates for bookmarks/smart_folder/cmdtemplate/session -- it differs only in target type (a raw HashMap<PathBuf,u64>-shaped cache, not a serde-derived settings struct) and base directory (dirs::cache_dir() vs fs_util::config_dir()). This module's own doc-comment states explicitly 'see ports::persist (Step 26) for the four-plus-this-one shared loader pattern' so Step 26's author does not miss this fifth instance purely because it landed 21 steps earlier.

7. Step 6 (A3, **done in the current ownership shape**): pure natural/composite ordering lives in `panel/sort.rs`. `SortColumn`/`SortOrder` remain shared panel vocabulary, while `ListingState::resort` is the only API that mutates row order and bumps its checked monotonic `ListingRevision`. The filter cache is already a private member of `ListingState`, keyed by that revision, query, facets, and time validity; extracting it into a second owner would weaken the invariant.

8. Step 7 (A3, **done 2026-07-28**): `ViewConfig` is a private-field, non-Serde value object owned by `ViewState`; callers use panel intents and read-only accessors. `PanelState` coordinates sorting, cursor preservation, and listing publication. Session conversion groups each panel's six persisted values behind one flattened adapter, preserving the legacy JSON keys, and passes both configs to `PanelState::new_with_view` before the first read. Hidden-file toggles stage a candidate read and commit config plus complete rows together; rejection preserves config, status, rows, selection/marks, cursor/scroll, size binding, watcher generations, listing revision, and filter-cache identity. Every command entry point publishes the typed outcome through the non-modal FIFO: success invalidates tree children and rejection produces shared feedback without disturbing modal ownership. Tree children are keyed by `(path, show_hidden)` and the cache is bounded.

9. Step 8 (A3, **superseded by shipped ownership**): do **not** pass `&mut entries_gen` through `ViewConfig` or expose entries/query/facets as parallel parameters to a detached cache. `entries_gen()` is now only a facade over `ListingState::revision()`. `replace`, `mark_incomplete`, and `resort` atomically bump the checked revision that invalidates the colocated cache. Focused tests cover exact revision bumps, stable ties, descending folders-first semantics, cursor preservation, hidden success/failure atomicity, watcher races, per-folder restore, legacy-compatible session bootstrap, typed command feedback, and bounded tree hidden-policy keys.

10. Step 9 (widened scope vs round 2, fixes the encapsulation-gap defect the critique found, CORRECTED for the test-write contradiction the reviewability lens found): extract panel::drag_state as its own struct PanelState composes, with a method API instead of public fields: take_drag() -> Option<(Vec<PathBuf>, Option<PathBuf>)> draining drag_entries+drop_target together, set_drag(paths), set_drop_target(path), clear() zeroing both in one call. Verified call sites beyond workspace.rs's drop_dragged/take_drop_plan (deferred to Step 22 since that rewrite is D22's dedicated fix-site): app/file_list.rs:146 (dragging check), app/file_list.rs:465 (drag_entries assignment on drag start), app/file_list.rs:480-481 (drop_target assignment), app/update.rs:93-94 (per-frame drop_target reset in begin_frame), and app/update.rs:845-867 (show_drag_overlay reading both fields). ALL FOUR of file_list.rs/update.rs's call sites are rewired to the new API in this SAME commit. CORRECTION vs round 3 (the reviewability lens's finding, verified: workspace.rs's own test module has drop_prefers_source_panel_target/drop_falls_back_to_other_panel_path/drop_with_conflict_opens_dialog_instead_of_moving at workspace.rs:2933-2934, 2985, 3000 writing `ws.left.drag_entries = vec![...]` and `ws.left.drop_target = Some(...)` directly as test setup): the plan's prior claim of 'ZERO remaining raw pub-field access anywhere in the crate except the one deliberately-deferred workspace.rs production site' was FALSE -- these three tests are additional raw-field call sites this step missed. Fix: fields become genuinely private (not pub(crate)) in this commit, and these three tests are REWRITTEN in this SAME commit to call `ws.left.drag.set_drag(vec![file.clone()])` / `ws.left.drag.set_drop_target(sub.clone())` instead of field assignment -- since drop_dragged's production body (still reading raw fields until Step 22) is a private method on PanelState itself, PanelState's own impl block can still reach its own private drag field directly without an API call, so the production deferral to Step 22 remains valid; only EXTERNAL (test and other-file) raw access is eliminated here, and the module's doc-comment states this distinction precisely: 'private field access from within panel::state's own impl (including workspace.rs's drop_dragged, which is itself a PanelState/Workspace method) is not a violation; the violation this step eliminates is access from outside the owning module, including test setup code.' This closes the gap outright instead of partially, and is exactly why D22's drag-and-drop rewrite (Step 22) gets a clean, invariant-enforcing baseline with zero remaining EXTERNAL raw-field access anywhere in the crate.

11. Step 10 (highest-risk panel.rs step, split into two independent commits, land each alone): (10a) extract panel::size_cache, owning dir_sizes/dir_counts/needs_refresh/sizes_dirty/last_sizes_recompute fields plus compute_dir_sizes (panel.rs:943-1094, fs_pool). Fix the clear-then-repopulate half of D5 here (race-safe generation counter or staged swap) since that bug is local to compute_dir_sizes. Expose narrow accessor methods -- `size_of(&self, path: &Path) -> Option<u64>` and `count_of(&self, path: &Path) -> Option<u64>` -- as the ONLY way any other module (including panel::overview and panel::selection_ops below) reads a cached size, rather than leaving dir_sizes/dir_counts's Arc<Mutex<HashMap>> reachable as raw fields even within the crate. (10b) extract panel::watcher, owning watcher/watched_path fields plus start_watcher (panel.rs:890-943) and the watcher-driven half of poll_fs_changes (panel.rs:856-890). Drop the redundant nested install() bug here since it is local to start_watcher. CLOSURE-COUPLING FIX (unchanged from round 2, still correct): start_watcher's notify::recommended_watcher callback closure directly writes needs_refresh/sizes_dirty (panel::size_cache's fields, via cloned Arc<AtomicBool> flags) AND calls invalidate_size_cache (panel::persist_cache's function) on every fs event. panel::watcher's dependsOn MUST list both panel::size_cache (for RefreshHandle/DirtyHandle accessor handles it clones into the closure) and panel::persist_cache (for invalidate_size_cache) -- add a doc-comment directly above the closure in start_watcher naming both cross-module writes. NEW FIX (the reviewability lens's third-timing-policy finding): poll_fs_changes (panel.rs:856-890) also reads a local SIZES_DEBOUNCE=500ms constant gating last_sizes_recompute -- a THIRD timing policy distinct from panel::watcher's own subscription-driven refresh trigger and panel::walk_log's WALK_COOLDOWN/WALK_EXPENSIVE (Step 5). SIZES_DEBOUNCE and last_sizes_recompute move onto panel::size_cache (they gate size recomputation, not the watcher subscription itself), and panel::size_cache's own module doc-comment lists all three timing policies by name (SIZES_DEBOUNCE here, WALK_COOLDOWN/WALK_EXPENSIVE in panel::walk_log, the watcher's own notify-driven trigger in panel::watcher) with one sentence stating they are three separate, uncoordinated 'don't redo this too often' mechanisms living in three files, so a future change to one does not assume it controls the others. panel::state composes both DirIndex-adjacent pieces as two separate fields (size_cache: SizeCache, watcher: DirWatcher) rather than one merged struct, since a watcher-API change and a size-algorithm change are orthogonal reasons to change -- but panel::watcher's own module-level doc comment must state plainly that it is NOT reviewable in isolation from size_cache/persist_cache's field semantics. Manually run the #[ignore]'d watcher_profile_harness after both commits.

12. Step 11: extract panel::listing (reload_entries, read_dir, subdirs) and panel::selection_ops (select_junk/select_largest/select_same_extension_as_cursor/select_empty_files/select_cursor/toggle_select/select_all/invert_selection/extend_selection/selected_entries/selected_or_cursor/select_by_mask/mask_match_count/listing_entries), each depending on the modules already landed in Steps 6-10. FIX vs round 3 (panel::listing's undeclared filter_cache coupling the reviewability lens found): reload_entries (panel.rs:827-848) calls self.filtered_get(...)/self.filtered_entries()/self.filtered_count() (three panel::filter_cache methods, now Step 8's explicit-parameter API) THREE TIMES, in addition to the already-documented sort_entries/entries_gen call. panel::listing's dependsOn now explicitly lists panel::filter_cache alongside panel::view/panel::sort/panel::entry/panel::dir_status, and this module's own doc-comment states both coupling facts together: 'reload_entries calls ViewConfig::sort_entries (bumping entries_gen) to re-sort after every disk read, THEN calls filter_cache's filtered_get/filtered_entries/filtered_count three times to restore the cursor position by path across the reload -- both calls are load-bearing, and a future edit reordering reload_entries's body must preserve entries_gen's bump happening before the filtered_* reads that depend on it being current.' FIX vs round 3 (panel::selection_ops's undeclared size_cache coupling the reviewability lens found, same defect class as panel::overview below): total_size_selected (panel.rs:1531-1561, explicitly named in this module's own movesFrom) calls panel::size_cache's size_of accessor (Step 10a) to resolve a selected directory's recursive size. panel::selection_ops's dependsOn now explicitly lists panel::size_cache alongside panel::filter_cache and panel::mask. Fix D18 (select_all should preserve filtered-out selections like invert_selection does) and D10's explicit (not silent-empty) selected_or_cursor miss in the selection_ops commit, since both are entirely local to that new module. Verify cargo test shows zero test-count change for both modules' moved slices.

13. Step 12 (RESEQUENCED vs round 2 -- fixes the ordering-deadlock defect the critique found): assemble panel::state as the slimmed PanelState shell BEFORE extracting panel::cursor_nav, not after, because cursor_nav's navigate_to/go_up (panel.rs:1163-1189) call self.refresh(), and refresh() (panel.rs:815-823) is a composition of reload_entries (panel::listing) + compute_dir_sizes (panel::size_cache) + start_watcher (panel::watcher) that only exists once panel::state's struct is assembled -- extracting cursor_nav as an independent leaf ahead of panel::state (as round 2's linear ordering implied) would leave it calling a refresh() that does not yet exist in reassembled form. This step: (a) assembles panel::state's struct (current_path, entries, view: ViewConfig, drag: DragState, size_cache: SizeCache, watcher: DirWatcher, cursor, selected, search_query, facets, entries_gen, filter_cache) plus construction (new/set_notify/has_notify) and refresh() -- refresh() now explicitly calls watcher.check(...) then size_cache.maybe_recompute(...) as two sequential steps (previously one function, poll_fs_changes) -- run the exact existing poll_fs_changes-driven tests before and after this reassembly, not just cargo test broadly, since this is a real (if small) control-flow change beyond a pure textual move; (b) in the SAME commit, extracts panel::cursor_nav (navigate_to, go_up, type_ahead, can_go_back/can_go_forward/go_back/go_forward) as inherent methods on the now-real PanelState, calling self.refresh() exactly as today, so there is no signature-cascade to invent here (refresh() is a zero-argument PanelState method both before and after, unlike ViewConfig::sort_entries's gen parameter, because refresh already lives on the struct that owns entries_gen). panel::cursor_nav's dependsOn is stated as panel::state itself (an inherent-method extraction, not an independent leaf) rather than listing size_cache/watcher/listing as if it could be extracted before them. Redistribute the panel.rs test module (panel.rs:1664-2557) into per-module #[cfg(test)] blocks now that the struct shape is final. Add one new regression test asserting filtered_entries() reflects a facet change made immediately after a reload_entries call.

14. Step 13 (ports, pulled forward ahead of the effects keystone because transfer_center/undo_center in Steps 17-18 need it): define ports::notify: a NotifyPort trait with an on_progress(&self) method, an EguiNotify impl wrapping the existing repaint-request closure, and a FakeNotify no-op for tests. This plan mandates ONE shape uniformly: Arc<dyn NotifyPort + Send + Sync>, cloned (cheap refcount bump) at every one of the ~9 call sites (start_transfer, pump_queue, poll_transfer, dismiss_transfer, perform_undo, perform_redo, execute_action, start_move_silent, gather_into_folder, drain_shelf, apply_sync). NEW FIX vs round 3 (the reviewability lens's adapter-closure finding, verified: transfer::spawn_transfer's actual signature is `notify: impl Fn() + Send + 'static`, transfer.rs:207-210 -- a generic monomorphized closure bound, NOT a trait-object parameter; Arc<dyn NotifyPort + Send + Sync> does not itself implement Fn() and cannot be passed there unchanged): spawn_transfer's own call site is the ONE place in the ~9 that needs an explicit adapter, not a bare Arc clone. This step's commit message and ports::notify's own doc-comment show the adapter verbatim: `let n = notify.clone(); spawn_transfer(..., move || n.on_progress())` -- an owned Arc clone moved into a zero-argument closure that satisfies spawn_transfer's existing Fn()+Send+'static bound by calling through to on_progress(). All other ~8 call sites (pump_queue, poll_transfer, dismiss_transfer, perform_undo, perform_redo, execute_action, start_move_silent, gather_into_folder, drain_shelf, apply_sync) take Arc<dyn NotifyPort + Send + Sync> directly as a parameter with no adapter needed, since none of them cross a generic-closure boundary the way spawn_transfer does. Workspace holds one Arc<dyn NotifyPort + Send + Sync> field and clones it into each call rather than choosing per-call-site between a borrow and an Arc.

15. Step 14 (A4 keystone, unblocked once panel is settled so Workspace's execute() has a stable panel API): FIRST, in a small preliminary sub-commit, define app::ui_state::dialog_kind: a standalone leaf module holding ONLY the DialogKind enum (Rename, Sync, Find, BatchRename, Treemap, RunCommand, Diff, Duplicates, ... one variant per dialog). NEW FIX vs round 3 (the DialogKind-ownership defect the critique found -- verified the round-2/3 plan text never states which module owns DialogKind, and either choice as stated has an unresolved problem: inside `effects` it makes the 'core' hard-code the UI's dialog catalogue, reproducing the exact defect the Effect bus exists to eliminate; inside `app::ui_state` it would need `effects`'s own Effect::Open variant to reference a type owned by a module downstream of it, an ordering inversion): DialogKind is deliberately NOT defined inside `effects` and NOT defined inside `app::ui_state::dialog_state_types` either -- it gets its OWN tiny leaf module (app::ui_state::dialog_kind, ~20 lines, zero logic, just the enum and its Debug/PartialEq derives) that BOTH `effects` and `app::ui_state::dialog_state_types` depend on downward, exactly the same shared-vocabulary-in-a-leaf pattern this round already applied to crate::kind (Step 3) for the SortColumn::Kind / query.rs / Kind-consumer problem. `effects`'s own dependsOn is updated to include `app::ui_state::dialog_kind` (a small, honestly-declared, one-enum dependency) rather than claiming zero UI-adjacent dependency -- the risk section explicitly notes this is effects' one deliberate exception to 'core has zero UI knowledge': it names WHICH dialog to open (unavoidable, since Effect::Open needs a discriminant) without naming ANYTHING about a dialog's field shape, validation, or rendering, which remain entirely in app::ui_state. Adding a new dialog means adding one variant to this ~20-line leaf, touched by both effects and ui_state's owner -- not editing effects' own logic. THEN introduce the effects module proper (Effect enum + one effects: Vec<Effect> queue field on Workspace) as an ADDITIVE field alongside the existing *_request fields -- do not delete the old fields yet. Add unit tests that execute() pushes the right Effect for 2-3 representative commands without wiring the UI drain. This isolates the risky UI-facing half into its own later commit.

16. Step 15 (A4, the keystone's risky half -- land alone, no bundling with A5 or A6): migrate execute() to push exclusively onto effects, delete the ~20 *_request fields and rename_target, and replace app/update.rs's ~11 mem::take drain sites with one drain_effects loop that dispatches on Effect and preserves the 'just opened' focus marker. Mine the shape from refactor/god-removal-ui-state's process_effects per architecture.md. NOTE the seam with Step 16 explicitly: rename_target (workspace.rs:125, deleted here) and RenameState (app/mod.rs:242-248, moved to app::ui_state::dialog_state_types in Step 16) are two halves of the same single-file-rename-dialog concern. This step's Effect::Open(DialogKind::Rename, just_opened, path) variant carries the target path as its payload explicitly, and this commit's message must say so, so Step 16's author knows the Open payload -- not a re-derivation -- is RenameState's snapshot-at-open-time source. NEW FIX vs round 3 (the tripled focused:bool idiom the critique found, verified: batch_rename_dialog.rs:103-106, find_dialog.rs:62-65, rename_dialog.rs:56-59 each duplicate `if !state.focused { resp.request_focus(); state.focused = true; }` backed by their own `focused: bool` field on BatchRenameState/FindState/RenameState at app/mod.rs:126,219,247): this step's Effect::Open carries an explicit `just_opened: bool` marker for exactly this purpose, and this commit's message states PLAINLY that Effect::Open's just_opened is INTENDED TO REPLACE the three per-struct focused fields, not coexist with them -- Step 16 (which moves the three structs) is responsible for actually deleting each struct's focused field and rewiring rename_dialog.rs/find_dialog.rs/batch_rename_dialog.rs's three request_focus() call sites to read the drained Effect's just_opened flag (passed down into each dialog's show function as a parameter) instead of the struct's own field. This eliminates the tripled idiom rather than adding a fourth 'just opened' tracker alongside it. cargo test guards the pure-core half only -- the dialog focus edge-trigger MUST be verified by hand in the running app for all three dialogs individually, exactly as architecture.md's own caveat states; do this before considering the commit done.

17. Step 16 (A5a): introduce app::ui_state::dialog_state_types (RunCommandState, FindState, DiffState, DupState, SyncState, BatchRenameState, RenameState struct defs and their builder methods, e.g. FindState::build_query, BatchRenameState::rule) as a pure struct relocation, EXCEPT for the focused:bool field deletion this step now owns (see Step 15's cross-reference): each of RenameState/FindState/BatchRenameState loses its own `focused: bool` field, and rename_dialog.rs/find_dialog.rs/batch_rename_dialog.rs's three request_focus() call sites are rewired to read a `just_opened: bool` parameter threaded in from the drained Effect::Open instead. RenameState gains its D19 snapshot-at-open-time field here, populated from the Effect::Open(DialogKind::Rename, just_opened, path) payload defined in Step 15 -- cross-referenced explicitly per that step's note. dialog_state_types depends on app::ui_state::dialog_kind (Step 14's small leaf, for RenameState's field typed against DialogKind if needed) and on effects only for the Effect type itself when draining -- NOT for DialogKind, which both modules get from the shared leaf, closing the ordering-inversion risk the critique found.

18. Step 17 (A5b): introduce app::ui_state::dialog_buffers, moving the ~15 dialog-buffer fields (renaming, type_ahead, mask_input, path_input, recent_input, palette_input, palette_tick, batch_rename, sync, duplicates, diff, treemap, find, saved_search_open, run_command) into one UiState struct App holds by composition. Also explicitly assigns smart_folders (Option<SmartFolders>, app/mod.rs:89), command_templates (Option<Templates>, app/mod.rs:93), and palette_usage (UsageStats, app/mod.rs:73) here -- lazily-loaded/persisted UI-adjacent state the dialogs and palette gate on, the same shape of concern as the buffer fields -- and app::app_struct's own movesFrom (Step 29) is updated to NOT claim them. Update every show_*_dialog call site to read/write through self.ui.*. Fix D19 (batch-rename studio and treemap dialog retargeting) in this same commit by giving each dialog's UiState variant a snapshot-at-open-time field populated when its Effect::Open is drained, since the fix and the extraction touch the same lines. ACKNOWLEDGED COUPLING (the critique's dialog_buffers-vs-dialog_state_types finding, addressed by honest disclosure rather than a re-split that would just relocate the same coupling): dialog_buffers's own fields (batch_rename, sync, duplicates, diff, treemap, find) are Option<T> wrappers around dialog_state_types's structs, so dialog_buffers really does structurally depend on dialog_state_types, and adding a field to e.g. BatchRenameState with a non-Default initial value still requires touching both modules' files in one commit (the struct definition in dialog_state_types, and App::new's initializer in dialog_buffers). This is stated explicitly in both modules' doc-comments as an accepted, common-case two-file commit -- the split is still justified because dialog_state_types's OWN internal logic (build_query, rule-application, snapshot population) genuinely does change independently of dialog_buffers's field list on other occasions (e.g. changing FindState::build_query's matching algorithm touches only dialog_state_types), just not for the 'add a field' case, which is disclosed rather than papered over. Land dialog_buffers as its own commit after dialog_state_types since it has the wider App-struct impact.

19. Step 18 (A6a): extract transfer_center as a narrowly-scoped module matching architecture.md's own definition -- queue + pump + poll + cancel + dismiss, PLUS the post-transfer panel refresh that today lives inline in poll_transfer/dismiss_transfer (verified at workspace.rs:875-948 and 979-987, both call self.left.refresh()/self.right.refresh()). NEW FIX vs round 3 (the ISP-widening defect the critique found: taking `&mut PanelState` grants unrestricted access to all ~15 of PanelState's fields -- selection, sort, filter cache, drag state, everything -- when poll/dismiss need exactly ONE capability, refresh()): define a narrow `Refreshable` trait in transfer_center's own module: `pub trait Refreshable { fn refresh(&mut self); }`, implemented by `impl Refreshable for PanelState { fn refresh(&mut self) { PanelState::refresh(self) } }` in panel::state (a one-line forwarding impl). TransferCenter::poll(&mut self, left: &mut dyn Refreshable, right: &mut dyn Refreshable, notify: Arc<dyn NotifyPort + Send + Sync>) -> Option<undo::Action> and TransferCenter::dismiss(&mut self, left: &mut dyn Refreshable, right: &mut dyn Refreshable, notify: ...) now take the narrow trait object, not the concrete wide struct -- this makes the panel dependency visible in the signature (closing round 2's DIP finding) WITHOUT granting transfer_center unrestricted access to PanelState's other ~14 fields (closing round 3's ISP finding on top of it). transfer_center's dependsOn now lists `panel::state::Refreshable` (the narrow trait) rather than `panel::state` (the wide struct) -- a future PanelState field addition/visibility change no longer forces transfer_center.rs to be re-reviewed, since it only ever sees a `&mut dyn Refreshable`. Workspace's thin wrapper becomes `if let Some(a) = self.transfers.poll(&mut self.left, &mut self.right, self.notify.clone()) { self.undo.stack.push(a) }` (unboxed &mut references upcast to &mut dyn Refreshable at the call site; PanelState already implements Refreshable) -- documented as the one place TransferCenter and UndoCenter are order-coupled (poll_transfer's undo-recording must run before pump_queue overwrites pending_undo_action). NOTE (the reviewability lens's poll/pump_queue-interaction finding): poll's own body already calls self.pump_queue(notify) internally as its last statement before returning (workspace.rs:875-949) -- the signature change from returning `bool` to `Option<undo::Action>` is not purely additive scope for the panel-refresh parameter; it also changes what the CALLER does with poll's return value while poll's internals still call pump_queue unconditionally. State this ordering explicitly in poll's own doc-comment: 'poll() calls pump_queue() internally before returning; the Option<undo::Action> return value is unrelated to and does not gate that internal call.' Verify against second_transfer_queues_and_runs_after_the_first, cancelling_a_transfer_drops_the_queued_jobs, dismissing_an_errored_transfer_retires_the_job_and_drains_the_queue before and after.

20. Step 19 (A6a-2, split out of transfer_center): extract workspace::transfer_requests, owning the PURE plan-computation half: request_copy/request_move/request_transfer/pending_conflicts/resolve_pending_conflicts. A Workspace-level glue module (peer to selection_glue/sync_glue), not part of transfer_center, because it reads both active_panel_ref() and inactive_panel() for plan computation -- a different concern from transfer_center's now-explicit post-transfer refresh -- and because 'decide what to transfer' is a different reason to change than 'run the transfer, then refresh the panels that just changed.' Unlike transfer_center, this module genuinely needs full PanelState (not just Refreshable) since it reads selection/entries/current_path to build a plan -- its dependsOn correctly states panel::state (the concrete struct), not the narrow trait. Calls transfer_center::start_transfer once a plan is confirmed and conflict-free. Verify against pending_transfer_overflow_logic and the existing conflict-resolution tests.

21. Step 20 (A6b, the one commit in A6 that changes control flow, not a pure move): extract undo_center, owning the UndoStack and perform_undo/perform_redo/execute_action/start_move_silent/dir_names/apply_rename_order, taking Arc<dyn NotifyPort + Send + Sync> per Step 13's corrected shape. Wire Workspace's thin poll_transfer wrapper per Step 18's line above. Move pending_undo_action itself onto TransferCenter in this same step (it is written by pump_queue/start_transfer and read by poll, both already TransferCenter methods). undo_center's own dir_names(dir) helper reads a directory's sibling names straight off disk via std::fs::read_dir -- CROSS-REFERENCE (the critique's fragmented sibling-name-lookup finding): this is the first of THREE independent 'collect sibling names' implementations that land across Steps 20-21 (this module's dir_names reading disk directly; fileops::batch_rename's active_dir_names reading the active panel's already-loaded entries; fileops::rename_single's inline siblings filter, which is active_dir_names minus the renamed entry). State in this module's own doc-comment: 'dir_names(dir) reads disk directly and is intentionally NOT unified with fileops::batch_rename's active_dir_names (Step 21), which reads the panel's in-memory entries instead of touching disk again -- the two have different correctness requirements (dir_names must see files sort_entries hasn't loaded yet; active_dir_names must match exactly what the UI shows) and merging them would introduce a footgun, not remove duplication. fileops::rename_single's siblings filter, however, IS just active_dir_names minus one entry and should call it rather than reimplementing the filter -- see Step 21.' This turns three previously-unlinked call sites into two deliberately-separate implementations plus one that explicitly reuses another, rather than three siloed copies with no note connecting them. Run the full transfer+undo suite (move_then_undo_restores_the_source, batch_rename_is_undoable_and_redoable, gather_into_folder_moves_selection_and_undoes) before and after -- this is the one A6 sub-step that cannot be verified by 'does it compile' alone.

22. Step 21: extract the fileops family from the remainder of workspace.rs, each its own small commit since they are now leaves against transfer_center/transfer_requests/undo_center, each taking Arc<dyn NotifyPort + Send + Sync> where they currently take a bare closure: fileops::mkdir (create_dir), fileops::delete (request_delete, exec_delete, trash_paths -- against ports::trash from Step 24), fileops::pending_op_confirm (confirm_pending_op), fileops::rename_single (commit_rename -- wire D20's new undo::Action::Rename variant here so F2 rename becomes undoable, in the same commit since rename_single and undo_center are both freshly touched; per Step 20's cross-reference, commit_rename's inline sibling-collision check calls fileops::batch_rename's active_dir_names() rather than reimplementing the same filter inline), fileops::batch_rename (batch_rename_targets, active_dir_names, apply_batch_rename -- active_dir_names is the shared helper Step 20/21 cross-reference each other over), fileops::gather (gather_into_folder, split internally into a pure plan_gather_folder name/path helper and the enqueue call -- fix the Gather-into-Folder orphaned-empty-folder-on-undo half of D20 here), fileops::duplicates (find_duplicates, confirm_dup_group). Verify each against its existing tests (apply_batch_rename_*, find_duplicates_groups_*, gather_into_folder_moves_selection_and_undoes).

23. Step 22: extract the remaining workspace glue modules, each a small independent commit since none share mutable state beyond &mut Workspace, each taking Arc<dyn NotifyPort + Send + Sync> where needed: workspace::shelf_glue (drain_shelf), workspace::sync_glue (build_sync_actions, apply_sync, enqueue_copy, start_copy), workspace::drop_glue (drop_dragged plus take_drop_plan, calling panel::drag_state's take_drag()/set_drop_target()/clear() API -- already the ONLY EXTERNAL API in the crate for this state as of Step 9, so this commit is a pure call-site rewrite with no remaining raw-field access to discover; this step also removes the pub(crate)-if-any visibility left over from Step 9's private-field boundary since drop_dragged is a PanelState-external caller once relocated to its own module -- confirm it now genuinely cannot compile without going through take_drag/set_drop_target -- and calling into transfer_center's queue for the actual move; this is the dedicated fix-site for D22's drag-and-drop rewrite: capture the actual dragged row explicitly, mirror drag state via panel::drag_state so the destination panel can render its own highlight, and call drag_state.clear() on the concurrency-guard early return so drag_entries/drop_target cannot desync into a phantom overlay), workspace::selection_glue (select_same_named, select_by_relation, stash_selection/combine_with_stash/stash_union/stash_intersect/stash_subtract/stash_symmetric_diff), workspace::bookmarks_glue (bookmark_dir plus the AssignSlot/BookmarkCurrentDir command handling), workspace::preview_glue (sync_preview, toggle_info). Each of these is one cohesive Workspace-level concern with its own reason to change; none are grouped merely because 'today's tests exercise them together.'

24. Step 23 (was workspace::queries, now four single-caller modules instead of one grouped module -- the original grouping's own responsibility text admitted 'used by exactly one dialog each,' i.e. zero shared reason to change): (a) fold treemap_items directly into the treemap dialog's own state area as a workspace::treemap_query leaf module (treemap_items alone) -- NOT bundled with run_find/reveal/diff_targets. (b) extract workspace::find_query as its own leaf module (run_find alone) and flag it, per D18's UI-thread note, as the landing spot for eventually moving this recursive walk off the UI thread. (c) extract workspace::reveal_nav as its own leaf module (reveal alone), grouped conceptually with cursor navigation despite living under workspace:: since it drives a panel's cursor from a cross-panel trigger (Find results). (d) extract workspace::diff_query as its own leaf module (diff_targets alone). Each of the four is under 40 lines; splitting them costs one extra file each but removes the false-cohesion module a change to Find's walk logic no longer risks touching a file whose other three functions are unrelated to Find.

25. Step 24 (interim checkpoint, fixes the 'execute() edited piecemeal with no visible shape' gap): before Step 25's final assembly, add a single doc-comment block at the top of execute()'s match statement (workspace.rs, still one file at this point) enumerating, arm-by-arm, which Command variants have already been repointed to Steps 18-23's modules and which remain inline -- update this comment in EVERY one of Steps 18-23's commits as part of that commit (a one-line diff each time). This gives each intervening commit's reviewer a visible artifact of execute()'s current intermediate shape. This is documentation-only, zero behavior risk.

26. Step 25: assemble workspace::mod as the slimmed Workspace struct (left, right, active, pending_op, transfers: TransferCenter, undo: UndoCenter, effects, opener, shelf, bookmarks, selection_stash, notify: Arc<dyn NotifyPort + Send + Sync>) plus active_panel()/active_panel_ref()/inactive_panel()/inactive_panel_mut()/new()/with_opener() and the execute() dispatch table (now a thin router pushing Effects or delegating one line per arm to the modules above, using Step 24's checkpoint comment to confirm every arm is accounted for before deleting it). This commit is almost pure deletion of already-moved fields/methods, verified against Step 24's running checklist. Redistribute workspace/tests.rs's remaining dispatch/navigation/stash tests to stay here; the transfer/undo/rename/fileops-specific tests move into their owning module's own test slice as each Step 18-22 commit lands. NOTE ON SRP: execute() as a top-level command router legitimately fans out to ~9 modules -- an accepted exception to single-reason-to-change for a dispatch table, bounded explicitly: if a future PR needs to add inline logic (not a one-line delegate) to more than 2-3 arms at once, that is the trigger to carve a new leaf module rather than grow execute() further.

27. Step 26 (A7a): define ports::trash (Trash trait: delete(&Path) -> Result<(), Error>, a RealTrash impl wrapping the trash crate, a FakeTrash for tests) and rewire fileops::delete's two trash::delete call sites (workspace.rs original lines ~1133, ~1419) through it, injected into Workspace the same way opener already is. NEW FIX vs round 3 (the ports::os_integration structural defect the critique found -- verified: native_menu.rs's action_get_info, at native_menu.rs:101-112, is a genuine `extern "C" fn(_: &Object, _: Sel, _: *mut Object)` AppKit action callback registered via `decl.add_method(sel!(actionGetInfo:), action_get_info as Fn)`; it has NO receiver capturing Workspace or any injected struct, and reads its target path from a thread_local, `MENU_PATH.with(|p| ...)` -- there is structurally no `self` for a trait-object port to be injected onto, and no path from Workspace's fields to a static extern fn): DO NOT introduce a ports::os_integration trait/impl/injection port for D12. D12 has now been fixed with a plain pure function `fn escape_for_applescript_literal(s: &str) -> String` (escaping backslashes and double-quotes, the two characters that break out of an AppleScript string literal) defined as a free function in native_menu.rs itself (or a tiny shared `applescript_escape` leaf if a second call site emerges later) and called directly from inside action_get_info's `with_path` closure before interpolating the path into the osascript command string: `format!("... POSIX file \"{}\" as alias)", escape_for_applescript_literal(&p.display().to_string()))`. This remains a pure function fix with unit tests, not a DIP abstraction -- no trait, no port, no injection, because the call site cannot structurally hold one. Do not fold native_menu.rs's own independent trash::delete call (native_menu.rs:146) into ports::trash or this commit; that is an unrelated one-line call-site change with no shared reason to change with the D12 fix, and is left as its own optional trivial follow-up (Step 26b, not required). ALSO FLAG EXPLICITLY (the critique's under-scoped-shell-outs finding): native_menu.rs has three MORE std::process::Command::new(...).spawn() shell-outs sharing the exact same swallowed-error (`let _ = ...`) pattern as the osascript call -- 'open -a' (line 81), 'qlmanage -p' (line 92), 'open -R' (line 140). These are explicitly OUT OF SCOPE for this step (they are not AppleScript-injection vectors -- Command::new with separate args, unlike osascript -e string interpolation, is not vulnerable to the same injection class), named here so a reviewer doesn't wonder why only one of four shell-outs was touched.

28. Step 27 (A7b): define ports::clipboard (Clipboard trait: copy_text; an EguiClipboard impl wrapping ctx.copy_text; a FakeClipboard for tests) and rewire the effects-bus clipboard drain in app/update.rs's drain_effects loop to call it instead of calling ctx.copy_text inline. crate::clipboard's existing PathStyle/format_path pure formatting is unchanged and reused as-is.

29. Step 28 (A7c): **shipped in the current persistence shape.** Lenient item stores share item-level recovery on load; Smart Folders and Command Templates now share `save_item_store`, which delegates to the durable atomic JSON boundary and records structured save failures while preserving their boolean UI contract. Session/bookmarks/versioned stores intentionally retain their stronger revision/envelope policies rather than being forced through this helper.

30. Step 29: split app/update.rs's per-frame rendering methods into small modules, each a mechanical cut of an already-self-contained method (no shared mutable state beyond &mut App): app/update::frame_bootstrap (begin_frame's setup half: repaint heuristics, notify wiring, poll_fs_changes, drop-target reset via panel::drag_state's clear()/set_drop_target() API from Step 9, handle_keys, preload_images), app/update::effect_drain (begin_frame's second half, now just the drain_effects loop from Step 15), app/update::dialog_drain (the eframe::App::update/save impl -- the ordered list of show_*_dialog calls, each now passed the drained Effect::Open's just_opened flag per Step 15/16's focus-idiom fix), app::quick_actions_glue (quick_action_context, run_quick_action, save_active_filter_as_smart_folder), app::focus_mode_glue (update_focus_mode), app/panels::toolbar_panel_render (show_toolbar_panel), app/panels::shortcut_bar_render (show_shortcut_bar), app/panels::shelf_tray_render (show_shelf_tray), app/panels::selection_hud_render (show_selection_hud), app/panels::main_area_render (show_main_area -- CHEAP INTERIM MITIGATION vs round 2/3, addressing the critique's 'no interim mitigation offered' finding: even though this module still mixes divider-width math, global tree-sidebar rendering, and compare-map cache reuse in one function and a full module split remains out of scope for this pass, THIS STEP splits show_main_area's body into three private helper functions in the same file -- render_divider(&mut self, ui) -> f32, render_tree_sidebar(&mut self, ui), and refresh_compare_cache(&mut self) -- each under ~50 lines, called in sequence from show_main_area's now-tiny body. This costs nothing, requires no new module/dependsOn edge, and means a future compare-cache bugfix's diff is contained to refresh_compare_cache's ~50 lines instead of spanning all ~165 undifferentiated lines -- still flagged as an accepted deferral of the FULL module split, not a resolution of the underlying SRP question, but no longer a zero-mitigation punt), app/panels::drag_overlay_render (show_drag_overlay, reading panel::drag_state's accessor methods already wired in Step 9 -- this is now a pure move of already-ported code, not a new rewiring), app/panels::type_ahead_overlay_render (show_type_ahead_overlay), app/panels::toast_render (show_toasts), app/panels::drop_input_render (handle_drop).

31. Step 30: split the remaining app/mod.rs construction/session logic into app::session_bridge (App::new's session-restore block and to_session) and app::init (the rest of App::new's construction), then assemble app::app_struct as the slimmed App struct (ws, ui, theme/zoom/tree/show_* toggles, image_cache, toasts, compare_cache) plus app::tree_glue (tree_expand_to_path) and app::confirm_outcome_toast (confirm_pending_op's toast formatting). app::app_struct's movesFrom does not claim smart_folders/command_templates/palette_usage -- those three fields were explicitly reassigned to app::ui_state::dialog_buffers in Step 17.

32. Step 31 (optional extension beyond committed Track A, not required to close it -- land only if reviewers want it after the above lands clean): split command.rs's palette-ranking machinery (command_catalog, CommandMatch, Usage, UsageStats, combined_score, rank, metadata_score, shortcut_search_text, command_aliases, filter_commands) into src/palette.rs, leaving command.rs holding only the Command enum and map_key/map_keys, which architecture.md already calls the 'clean' dispatch direction. Note UsageStats itself is also referenced by app::ui_state::dialog_buffers's palette_usage field (Step 17) -- palette.rs and app::ui_state::dialog_buffers share a type, not a module boundary; no change needed, just flag the dependency for the reviewer.

33. Step 32 (optional extension beyond committed Track A, explicitly requested on 2026-09-01): **method-tabs extraction shipped** as a state-independent `current -> Option<selected>` UI component. The two list-rendering strategies (`render_flat_list_virtual`, `render_flat_list_animated`) remain candidates for `app/confirm_dialog/flat_list.rs`; move them only when their shared row model can be expressed without coupling the leaf back to `App`.

#### Risks called out by the design pass

- Kind/kind_of's split into its own crate::kind leaf (fixing round 2's incomplete Kind/kind_of move) touches 8 call sites in one commit (query.rs, file_color.rs, smart_folder.rs, app/find_dialog.rs, app/mod.rs's FindState, app/treemap_dialog.rs, app/file_list.rs, panel.rs's SortColumn::Kind arm) instead of the 1-2 round 2 implied. This is a wider single commit than a typical panel.rs leaf extraction, but splitting it across multiple commits would leave the crate in a state where some call sites import from selection_summary and others from kind, which is worse than one larger, purely mechanical, compiler-verified commit. Grep for `selection_summary::Kind` and `selection_summary::kind_of` after landing to confirm zero remaining references outside selection_summary.rs's own (now delegating) use of kind_of.
- panel::cursor_nav's extraction was resequenced to land in the SAME commit as panel::state's assembly rather than as an independently-movable earlier leaf, because navigate_to/go_up call self.refresh(), and refresh() (a composition of reload_entries+compute_dir_sizes+start_watcher) does not exist in reassembled form until panel::state's struct is composed. This makes panel::state's own commit larger (struct assembly + cursor_nav's inherent methods together) than round 2's plan implied, but resolves a genuine ordering deadlock the reviewability lens found: extracting cursor_nav first, as round 2's linear step ordering suggested, would leave it calling a not-yet-composed method. Reviewers should expect this one commit to be the single largest 'reassembly' commit in the panel.rs track.
- transfer_center's Refreshable trait is a new abstraction not present in architecture.md's literal text (which says TransferCenter takes 'queue+pump+poll+cancel+dismiss'), introduced specifically to avoid granting transfer_center unrestricted &mut access to all of PanelState's ~15 fields when it only ever calls refresh(). The trait is one method, implemented by a one-line forwarding impl on PanelState, so the runtime/compile cost is negligible, but it is still a second seam (beyond ports::notify) invented by this plan rather than dictated by the existing codebase or architecture.md -- worth a reviewer's explicit sign-off that a narrow capability trait is preferred over the wider concrete-struct parameter round 2 used, on ISP grounds.
- DialogKind's placement in a brand-new tiny leaf (app::ui_state::dialog_kind) that BOTH effects and dialog_state_types depend on downward is a deliberate resolution of an ownership question architecture.md's text does not address at all -- the plan's position is that effects depending on a ~20-line, logic-free enum (naming which dialog to open) is a bounded, honest exception to 'core has zero UI knowledge,' not a violation of the Effect bus's purpose, since nothing about a dialog's FIELDS, validation, or rendering lives in that enum or in effects itself. A reviewer who disagrees with this framing should flag it before Step 14 lands, since reversing the decision later (e.g. moving DialogKind fully into ui_state and having effects import upward) is a larger, ordering-sensitive change once Effect::Open's payload type is baked into ~20 call sites.
- ports::os_integration was fully discarded and replaced with a plain pure function (applescript_escape) after verifying that native_menu.rs's action_get_info is a static `extern "C" fn` AppKit callback with no receiver to inject a trait object onto -- it reads its target path from a thread_local, not from any struct. This is a case where the round 2/3 plan's own DIP instinct (wrap this in a port, like opener) was simply the wrong tool for a call site with no `self`; the corrected plan ships a one-function fix with a unit test instead of an unused trait. Reviewers should confirm no OTHER AppleScript/osascript call site exists anywhere in the crate that WOULD benefit from a real port (grep for `osascript` found exactly one call site, native_menu.rs:101-112, at the time of this plan).
- ports::persist grew from one helper (load_lenient<T:Default>) to three (load_lenient, load_optional<T> for session.rs's Option<Session>-returning, no-Default case, and save_atomic<T> for the four-way-duplicated save side) after verifying session.rs's load() genuinely does not fit the single-signature framing round 2 proposed, and that the save-side duplication is exactly as real as the load-side duplication round 2 already targeted. Three small helpers instead of one is slightly more surface area to review, but each is a straightforward 10-20 line generic function; the alternative (forcing session.rs to adopt a Default impl it doesn't have today, purely to fit one helper's signature) would be a behavior change disguised as a refactor, which is worse.
- poll_transfer/dismiss_transfer straddle transfer_center, undo_center, AND panel::state::Refreshable by design today (workspace.rs:875-948 and 979-987 both call self.left.refresh()/self.right.refresh() in addition to the undo-recording read-before-pump ordering). This plan resolves both the panel coupling (via the explicit Refreshable parameter) and the ISP over-widening (via the narrow trait instead of the concrete struct) -- but poll()'s own internal, unconditional call to pump_queue() before returning is a separate, pre-existing control-flow fact this plan does NOT change, only documents explicitly in poll's own doc-comment so its Option<undo::Action> return-value change is not mistaken for altering that internal ordering. Run the full transfer+undo suite by hand, not just cargo test, both before and after this step.
- Historical risk note for the Effect-bus plan: egui frame timing originally had no automated coverage. The shipped `UiRequest` implementation closes most of that gap with headless tests for FIFO modal ownership, deferred multi-frame dispatch, IME/text focus, payload identity, SafeState-to-Recovery handoff, and transition-frame Escape. A running-app pass is still required for native-window focus and visual layering, but the claim that the suite cannot verify the boundary at all is no longer true.
- ports::notify's abstraction shape is fixed as Arc<dyn NotifyPort + Send + Sync> uniformly for ~8 of ~9 call sites, but spawn_transfer's own `impl Fn() + Send + 'static` bound requires ONE explicit adapter closure (`move || n.on_progress()`) rather than a bare Arc clone -- this is a real, if small, asymmetry in the ~9 call sites this plan now states explicitly rather than leaving implicit. Confirm at Step 13's landing that this one adapter closure is present and tested (or at minimum exercised by the existing transfer-completion tests), since a missed adapter at this one site is a compile error, not a silent bug, so the risk here is review clarity rather than correctness.
- panel::watcher's fs-event closure crosses three module boundaries at closure-construction time (writes panel::size_cache's needs_refresh/sizes_dirty flags via RefreshHandle/DirtyHandle accessors, and calls panel::persist_cache's invalidate_size_cache directly) -- this round additionally documents panel::size_cache's own SIZES_DEBOUNCE timer as a third, previously-unnamed timing policy in the same subsystem (alongside panel::walk_log's WALK_COOLDOWN/WALK_EXPENSIVE and panel::watcher's own notify-driven trigger), giving a reviewer a complete three-policy inventory across three files instead of the two the plan tracked before. This is a documented, narrowed coupling, not an eliminated one -- a reviewer of any future change to panel.rs's timing/debounce logic must still read three modules' doc-comments together, and that residual review cost is accepted rather than solved.
- panel::drag_state's privatization commit now explicitly includes rewriting three workspace.rs test-module call sites (drop_prefers_source_panel_target, drop_falls_back_to_other_panel_path, drop_with_conflict_opens_dialog_instead_of_moving) in addition to the five production call sites round 2 already accounted for -- a genuinely larger single commit (production code in three files plus test code in a fourth) than a pure 40-line struct extraction, landing in one commit specifically so the crate compiles with zero EXTERNAL raw-field access at any intermediate point. Reviewers should expect this commit's diff to touch four files, not three, and should re-verify by grepping for `.drag_entries` and `.drop_target` outside panel::state's own module after this step lands -- the only remaining hits should be inside workspace.rs's drop_dragged/take_drop_plan PRODUCTION bodies (deferred to the drop_glue step), not test code.
- This plan still deliberately exceeds the 200-400 line target for a few modules (panel::selection_ops ~260, effects ~205, workspace::mod ~260, transfer_center ~230, panel::state ~235 after absorbing cursor_nav's resequenced methods) where a bounded context's public API would otherwise be split with no independent caller today. panel::state's line count grew from round 2's ~205 estimate specifically because cursor_nav's methods now land in the same commit/module rather than as an independently-extracted sibling -- an honest accounting increase driven by the ordering fix, not scope creep. The remaining oversized modules are flagged to reviewers as deliberate, mirroring architecture.md's own '~600 line' ceiling rather than the task's tighter 200-400 target.
- Redistributing workspace/tests.rs's ~1,300 lines and panel.rs's ~900 lines of existing tests across the new module boundaries remains the largest mechanical-but-error-prone activity in the whole plan. Move verbatim first, then redistribute test-by-test as each owning extraction commit lands, keeps every commit's test delta reviewable against that commit's code delta, but a few tests that exercise multiple new modules together will need to be either duplicated or left as an integration-style test in workspace::mod; flag this as a per-test judgment call, not a mechanical script.
- native_menu.rs's D12 fix (applescript_escape) leaves three OTHER shell-out call sites in the same file with the same swallowed-error (`let _ = ...`) pattern explicitly out of scope and explicitly named as such ('open -a' at line 81, 'qlmanage -p' at line 92, 'open -R' at line 140) -- they are not AppleScript-injection vectors since Command::new with separate args is not vulnerable to the same string-interpolation injection class osascript -e is, so leaving them untouched is a defensible scoping choice, not an oversight, but it is worth a reviewer's explicit acknowledgment that 'fix D12' means fixing exactly one of four shell-outs in this file by design.
- Naming/scope conflict resolved explicitly: folding native_menu.rs's AppleScript 'Get Info' shell-out into ports::trash (as an early draft proposed) conflates two different capabilities under one port name; this round goes further and removes the port framing for the AppleScript fix entirely (see the applescript_escape entry) rather than inventing a second, still-awkward port. native_menu.rs's own unrelated trash::delete call (line 146) remains untouched by both the D12 fix and ports::trash's own introduction -- rerouting it through ports::trash for consistency, if desired, is its own trivial follow-up with no shared reason to change with either.

## Validation: the `refactor/god-removal-ui-state` spike

A divergent branch, `refactor/god-removal-ui-state`, has already prototyped this
direction and more: a full Effect bus (the `Requests` struct removed, effects
processed in one `process_effects`), a `UiState` extraction, `Pane` /
`CommandHandler` traits with a plugin registry, a tokio runtime with
`spawn_blocking`, a virtualised file list, plus net-new features (tabs, git
status, tags, notes, configurable columns). It sits on a stale base (`main` is
~42 commits ahead, the branch ~13), so a direct merge is conflict-heavy.

**Decision (see [recommendation.md](recommendation.md)): `main` stays the
mainline; the spike is treated as a proven reference, and its ideas are
re-landed onto `main` in small, reviewable steps** rather than merged
wholesale. The Effect-bus and `UiState` designs above are exactly what the spike
validates.

## Invariants and testing

The default suite includes three explicit ignores, including the separately
executed single-threaded CI performance gate. Headless egui/AccessKit tests
exercise text-focus and IME suppression, modal priority, FIFO pending
ownership, one-shot Escape routing, and the SafeState-to-Recovery transition
frame. A feature-gated native QA process provides four eframe/Glow framebuffer
scenarios. Desktop, minimum-window, 200% accessible, and confirmation-modal
captures are all strict CI matrix jobs. Every process uses a temporary storage
root and fake native ports, always writes capability/manifest JSON and writes
PNG when framebuffer readback is available, then checks frame diversity,
logical viewport size, geometry, pane separation, modal stacking policy,
painted glyph pixels, and absence of native effects.

The native context menu is a declarative `MenuInvocation` tree with stable item
IDs and an invocation-bound target. AppKit renders that model with
invocation-local handler state; Objective-C selectors only record a typed
selection. Duplicate, Compress, Tags, and Share execute after menu tracking
returns through `ContextMenuPort::perform_deferred_action`. This removes the
former thread-local path/result state, prevents callbacks from mutating files
or launching services, balances owned AppKit menu objects, and preserves
pathname bytes through NSURL filesystem representations.

`native_release_qa` is a narrow facade over six owners: `contract` defines
evidence types, `policy` owns pure placement/attestation/verdict rules,
`macos_probe` reads already-granted capabilities and `NSScreen` topology,
`identity` owns compile-time build provenance plus canonical topology
fingerprints, `secure_artifact` owns bounded no-follow reads and private atomic
writes, and `artifact` only orchestrates evidence assembly. `build.rs` watches
Git HEAD/index/ref and every tracked package file, so a dirty build cannot
become an apparently clean stale binary after its source edit is reverted.
Runtime environment commit hints are not trusted. The subject also requires
the current process's kernel CDHash to match a strictly verified code signature
for the exact file whose BLAKE3 is recorded.

The production AppKit bridge captures the exact pointer or row anchor during
the input frame, reads actual `NSMenu.size` after dynamic provider discovery,
refreshes display topology at show time, and asks the same pure policy for a
top-left placement whose complete downward-growing content rectangle fits one
`visibleFrame`. Oversized menus fail closed. Secondary click, Shift-F10, and
AccessKit `ShowContextMenu` store the exact listing identity, trigger, anchor,
pane, and row focus in `UiState`; an AX request activates its pane and publishes
the new cursor/focus tree before synchronous AppKit tracking. Latest-wins
replacement, modal/window-focus cancellation, and post-tracking focus return
are reducer contracts. Directory rows no longer publish a false
`expanded=false` state.

The automated framebuffer still excludes AppKit's separate popup and window
chrome. Actual `NSMenu` model/renderer introspection and full-rectangle
center/corner placement are automated without TCC. Popup pixels, VoiceOver
speech and task navigation, Escape focus return, and real multi-monitor
interaction are human checks bound to the exact commit, executable BLAKE3 and
topology fingerprint. Strict policy rejects dirty or mismatched builds, unknown
JSON fields, missing/duplicate/extra cases, missing permissions, empty review
fields, stale/future attestation, `not_run`, mismatches and blocked checks.
Diagnostic mode records them without prompting or claiming pass. Evidence and
attestation files use private descriptor-relative atomic writes; path symlinks
and non-regular inputs are rejected.
The shipping renderer remains WGPU; its eframe 0.35 Metal screenshot readback
is a manual boundary because external `Device::poll` attempts can deadlock the
renderer/event-loop ownership. A missing screenshot event is a test failure,
not a capability skip.
