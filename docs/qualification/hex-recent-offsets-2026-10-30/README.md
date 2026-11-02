# Hex Go expression history — 2026-10-30

Independent adaptation of [Far Manager dialog edit-control history](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4) (`MiscCmd`, `DialogSettings`), cohort #37 in the 99-project research. Far supports recalling a previous string into its input control; this transfer adds that behavior to the existing hex Go field.

## Implemented

Recent offsets stores up to 20 exact Go expressions, newest first with exact-text deduplication. Recording happens only after the existing read-workload submission succeeds. Invalid syntax, invalid relative bounds and busy inputs do not add entries; later worker errors or cancellations retain an admitted expression. Absolute offsets beyond EOF are accepted by the existing parser then rejected by the worker, and therefore remain in history. Numeric aliases remain distinct strings. UI input remains capped at 32 characters; the history owner also caps an entry at 256 UTF-8 bytes (at most 5 KiB total retained text).

Choosing an entry restores only the Go input, clears its stale error and focuses the field. It performs no read and preserves the loaded page, position history, search settings and bookmarks. Pressing Go evaluates the recalled expression again: relative expressions use the current loaded page and percentages use the current file size. Clear affects only recent Go entries. Reload retains this input history; closing the viewer discards it. Existing Back/Forward position history remains independently owned.

## Evidence

Two regressions exercise real deterministic workers: admission/bounds/deduplication/eviction/busy suppression/Reload, and egui pointer recall followed by an actual Go button click, preserving unrelated state and reevaluating a relative expression. Initial test expectations incorrectly classified an absolute beyond-EOF address as a parse rejection; the rejected logs are retained. The corrected case explicitly checks admission followed by worker error rather than changing existing parsing behavior.

The native HexPreview scene retains its previous pagination, radix, percent, display and bookmark actions and then opens Recent offsets, recalls 100% without reading at position zero, and presses Go to reach EOF 8197. That extension is prepared but native execution is pending: no CUA tool is exposed in this continuation. Earlier captures do not qualify this binary or the new menu. No native/OS clipboard claims are made.

Final checks and source hashes are recorded alongside this report after completion.

Validation: the full visual-QA test suite passed 2797 tests, with 3 ignored. Final-source Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. A final QA-only predicate distinguishes the recalled expression (before Go) from the most recently submitted expression (after page acceptance canonicalizes the input); that prepared native predicate is compile-checked but remains unexecuted. No commit or push performed.
