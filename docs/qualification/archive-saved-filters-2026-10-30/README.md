# Archive saved query settings — 2026-10-30

Independent adaptation of [Far Manager dialog input history and retained frequent strings](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4) (`MiscCmd`, `DialogSettings`), cohort #37 in the 99-project research. This transfer provides explicit saved query/settings snapshots for the existing archive member filter, rather than recording every live edit.

Save filter records the exact query, Regex/Match case/Smart case, Exclude matches and all type flags. Invalid or over-256-character queries are rejected by the same bounded parser; a completely default empty filter is not saved. Empty-query snapshots with meaningful options/type/exclusion settings remain supported. Exact full-snapshot duplicates move to the front. At most 20 snapshots and 16 KiB of raw UTF-8 query text are retained, evicting oldest saves first.

Saved filters uses escaped/truncated query labels and readable settings with full hover details. Recall restores only query/settings and invalidates the filter cache so the existing visible-row owner recompiles and rebuilds immediately. It applies to the current folder/narrowing and retains sorting, current/saved selections and folder navigation history. Caller restores filter focus. No source read, decode, extraction or worker task is started by this owner.

Reset filters retains saved entries. Clear saved filters changes only the saved-entry deque. All snapshots are owned by the current listing's MemberFilter; publication of a reloaded listing or a new/closed archive discards them. This is session storage, not persistent named presets or a full Far dialog-history implementation.

Three regressions cover full-key deduplication, entry/text bounds, invalid-regex/length rejection, restoring actual visible rows while preserving location/narrowing/sort/selection-memory owners, and real egui pointer recall of query/Regex/case/exclusion/types. Native Save/menu/clear/focus and wrapped panel layout remain unqualified: CUA is not exposed in this continuation, no native launch was attempted, and earlier frames do not qualify this source.

Validation: the locked visual-QA suite passed 2805 tests (3 ignored). Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
