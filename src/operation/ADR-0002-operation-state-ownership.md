# ADR-0002: Operation State Ownership

- Status: Accepted
- Date: 2026-07-14
- Scope: module boundaries for operation planning, execution, and presentation

## Context

The same operation appears in the workspace queue, transfer worker, durable
journal, undo history, recovery center, progress dialog, and support bundle.
If each surface owns a mutable copy of status or policy, those copies diverge
under cancellation, crash recovery, or late worker completion.

## Decision

Ownership is assigned by concern:

| State or decision | Owner | Non-owner responsibilities |
| --- | --- | --- |
| Operation IDs, durability, and failure classes | `operation.rs` | Other modules carry these typed values unchanged |
| Serializable operation and step status | `operation_journal.rs` | Executors submit typed events; UI reads projections |
| Source/destination identity evidence | `path_identity.rs` | Journal persists snapshots; executor revalidates them |
| Filesystem names and capability policy | `filesystem_policy.rs`, `volume_profile.rs`, `mount_guard.rs` | Workspace requests preflight; dialogs explain the verdict |
| Staging, byte production, verification, and placement | `transfer.rs` plus copy/hash backends | Journal records checkpoints and outcomes |
| Priority, quota, cancellation, and stale generations | `workload.rs` | Workers cooperate through the supplied token |
| Queue order and requested user command | `workspace.rs`, `opqueue.rs` | They do not manufacture durable completion states |
| Undo eligibility and replay plan | `undo.rs` with journal evidence | UI requests and confirms; it does not mutate history early |
| Versions replaced by a committed operation | `version_store.rs` | Recovery renders and restores verified records |
| Labels, progress projection, and interaction | `operation_view.rs`, `app/` | Presentation cannot weaken policy or write journal state directly |

The durable journal is the source of truth after process restart. Live worker
state may be richer while the process is running, but it cannot retroactively
declare an unjournaled effect committed. Immutable task snapshots prevent a
late worker from publishing into a newer UI generation.

## Dependency Direction

Core operation modules do not depend on egui or dialog state. The UI may depend
on typed core projections. Copy backends do not own policy. Provider and
filesystem effects are reached through narrow ports or executor-owned helpers,
not through presentation callbacks.

## Consequences

- A new operation surface should project existing state instead of introducing
  another status enum.
- A new backend should implement byte/effect mechanics without taking ownership
  of conflict, durability, or terminal-state policy.
- `Workspace` and `App` remain coordination hot spots, but operation safety does
  not move into either while they are being decomposed.
- Tests can exercise transition and recovery policy without constructing egui.
