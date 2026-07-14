# ADR-0001: Durable Operation Invariants

- Status: Accepted
- Date: 2026-07-14
- Scope: copy, move, sync, undo, redo, and recovery placement

## Context

A file operation can report a successful syscall while still leaving the user
with partial data, a stale destination, or an unrecoverable source. Local,
remote, removable, case-sensitive, case-insensitive, and remounted filesystems
also expose different placement guarantees. UI confirmation cannot make these
differences safe after execution has started.

## Decision

Every durable operation obeys these invariants:

1. Data is written to hidden sibling staging. The requested destination is not
   exposed as complete while bytes are still being produced or verified.
2. Source, destination, landing, and staging identities are observed and
   revalidated at their decision boundaries. A changed identity invalidates the
   reviewed plan.
3. The volume generation and capability profile used by preflight remain part
   of the execution contract. A remount or capability change fails closed and
   requires a fresh review.
4. Each step has an idempotency key. Replaying a persisted step may reconcile a
   completed effect, but must not apply that effect twice.
5. A resume checkpoint names the same staging object and a verified logical
   boundary. Uncheckpointed tails are discarded before continuation.
6. Final placement happens only after the selected durability policy succeeds.
   `Verified` and `Versioned` operations require end-to-end verification;
   `Versioned` also preserves the replaced destination before placement.
7. A no-replace placement never silently overwrites a newly occupied name.
   Conflict policy is re-evaluated against the current destination identity.
8. Operation and step statuses change only through their typed transition
   events. A terminal label is evidence of an accepted transition, not an
   arbitrary field assignment.
9. Rollback removes only effects whose identity still matches the journal. It
   never deletes an unrecognized file merely because the path matches.

## Consequences

- Fast paths such as rename, clone, sparse copy, resume, and delta remain
  optimizations beneath one placement contract.
- Some operations stop for review instead of guessing after a remount,
  collision, integrity uncertainty, or identity race.
- The journal contains enough state to reconcile a crash immediately before or
  after each filesystem side effect.
- New copy backends must prove the same invariants; successful byte production
  alone is insufficient.

## Verification

[`operation_verification.rs`](../operation_verification.rs) injects faults before
and after typed filesystem effects, serializes and restarts at every journal
transition, and checks copy/move/sync conflict combinations for no-loss
properties. Transition legality is exhaustively tested in
[`operation_journal.rs`](../operation_journal.rs).
