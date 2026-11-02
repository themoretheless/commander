# Remove one saved archive filter — 2026-10-30

Independent adaptation of [Far Manager individual dialog-history deletion](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4) (`MiscCmd`, Shift+Del), cohort #37. Commander exposes the action as a Remove button beside each saved filter in its popup; no new keyboard binding is claimed.

Removal identifies the complete immutable snapshot (query, pattern/case flags, type flags and exclusion), rather than query text or a mutable row index. Other snapshots with the same query remain in order. Removing a stale/missing snapshot does nothing. The active query, compiled/cache state, cursor/range anchor, location and saved/live selection owners are retained. Removing the last saved entry disables the saved-filter opener. Existing Clear saved filters and recall remain available. Disabled controls cannot mutate the list.

The row reserves a bounded label area and a separate Remove action; query labels remain truncated with readable full hover details. Clicking Remove closes the popup without invoking recall or resetting the filter's focus owner.

Two new regressions verify exact-snapshot removal, repeat/missing removal, preservation of active cache/cursor/range/selection memory and pointer removal without query recall. The existing pointer-recall regression also passes with the changed row layout. All five focused saved-filter tests pass.

Rejected attempts are retained: `test-initial.log` is a test type-inference error; `test-context-initial.log` and `diagnostic.log` show that a context menu nested in the popup rendered only once as its parent closed, leaving a stale action rectangle and no mutation; `test-row-layout-initial.log` rejects the first right-to-left row arrangement because neither pointer action worked. The final horizontal row with a bounded label area passes both pointer paths. Temporary diagnostic hooks were removed.

Native menu/action/layout/focus remains pending: CUA is not exposed in this continuation, no native launch was attempted, and prior frames do not qualify the changed source.

Validation: the locked visual-QA suite passed 2807 tests (3 ignored). Final-source Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
