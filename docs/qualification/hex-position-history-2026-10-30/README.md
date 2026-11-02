# Hex successful-position history — 2026-10-30

Independent adaptation of [Far Manager viewer Undo position change](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Verified upstream documents position undo. Commander independently provides Back and Forward buttons; exact upstream keymap/history policy parity is not claimed.

Every successfully accepted page records its exact byte offset, covering First/Previous/Next/Last, typed absolute/relative Go, bookmarks and search-hit pages. Back/Forward request a guarded reread of a saved position. The stack moves only after the resulting page is accepted; failed, rejected, cancelled or unsubmitted reads do not consume saved positions. New successful navigation from a historical point discards Forward. Repeated visits to the same current offset add no duplicate. Search settings, byte representation and bookmarks remain current; returning to a search-hit offset does not recreate match highlighting or restore its query. No pixel-scroll or arbitrary cursor state is saved.

Past and future together hold at most 64 u64 offsets, plus the current offset. Oldest past entries are pruned first, then oldest future entries. Back/Forward are disabled during work or when their stack is empty. They can recover a saved position after an error/cancel, but still use existing file-binding guards; a changed source requires Reload. Reload clears history and bookmarks and admits the current source anew, retaining existing behavior. A busy Reload is ignored before any state reset. Closing/replacing the viewer discards history. No persistence or extra prefetch is added.

Existing page-acceptance, bound descriptor/path metadata checks, workload ownership and cancellation remain authoritative. History stores only offsets, not page buffers; revisits queue the same bounded 4 KiB read as normal navigation. Shared cancellation cleanup now also clears the pending history request. History current means the last successfully accepted position, even if a later error/cancel cleared the displayed page.

Three regressions cover 64-point bidirectional bounds, duplicate suppression, branching and mismatched commits; actual deferred commit, cancellation, changed-source rejection and Reload reset; real search-hit history, Back reread and branch replacement with query retention. Existing pointer/search/bookmark tests remain part of the full suite. UI wiring is inspected; no native pointer/key validation is claimed.

Native Back/Forward click, failure recovery and layout validation remain pending because desktop CUA is unavailable. Historical hex captures do not qualify these controls/current binary; earlier native gates remain open. No native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2788 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied passed (`clippy.log`).
- Formatting and whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked default check and visual-qa build passed (`check.log`, `build.log`). Source/binary hashes and pending native-validation record accompany this report.
