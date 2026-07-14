# ADR-0003: Failure and Recovery Policy

- Status: Accepted
- Date: 2026-07-14
- Scope: cancellation, retry, uncertain integrity, rollback, and diagnostics

## Context

Errors differ in what the system can safely do next. Treating every error as a
retry can duplicate effects; treating every error as terminal strands resumable
work; treating uncertain integrity as an ordinary warning allows later
mutations to destroy evidence needed for recovery.

## Decision

Failures use four stable classes:

| Class | Policy | User surface |
| --- | --- | --- |
| `Retryable` | Retry only within an explicit attempt and backoff budget; retain the same idempotency key | Progress and queue show retry/requeue state |
| `Blocked` | Stop the affected step; preserve completed evidence and staging | Error inbox offers retry after the external condition changes |
| `UserDecision` | Stop before choosing overwrite, skip, keep-both, normalization, or portability policy | Conflict/review UI presents explicit alternatives |
| `IntegrityUncertain` | Fail closed, preserve evidence, and block related mutations until reviewed | Safe state and Recovery Center lead the workflow |

Additional rules apply:

1. Cancellation is cooperative and bounded. A worker observes its scheduler
   token at safe checkpoints, persists recoverable progress, and cannot publish
   a stale generation.
2. Retry counts are finite. A disconnect, remount, source change, or repeated
   integrity failure cannot create an infinite retry loop.
3. Completed steps remain visible when a later step fails. The operation is not
   summarized as wholly successful or silently reset to planned.
4. Recovery first reconciles journal evidence with current identities. It may
   resume, retry, roll back, clean a recognized orphan, or require review; it
   does not guess from path existence alone.
5. Rollback is another journaled operation over known effects. Partial rollback
   remains explicit and recoverable.
6. Risky content-index, image-preview, and external-provider paths have runtime
   kill switches and stable bounded rollout cohorts. Disabling one prevents new
   work at its owning boundary.
7. Capability diagnostics may contain explicit user-selected paths. Support
   bundles may not: paths, operation IDs, version keys, and failure text are
   replaced by per-bundle salted tokens or aggregates.
8. Errors are never discarded because a dialog closes. The Operations Center,
   journal, and redacted support bundle retain the appropriate projection.

## Consequences

- Some recoverable work waits for idle time, reconnection, or user review
  instead of consuming resources in a retry storm.
- Integrity uncertainty is deliberately more disruptive than an ordinary I/O
  failure because preserving evidence has priority over throughput.
- Diagnostics can explain selected fallbacks without making private operation
  details part of routine telemetry.
- New failure kinds must map to one of these policies or amend this ADR and the
  transition tests in the same change.
