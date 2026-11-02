# Hex recent search history — 2026-10-30

Independent adaptation of [Far Manager dialog edit-control history](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Verified upstream describes recalling input history and clearing it; Commander independently snapshots hex search query, input mode and Wrap rather than claiming pinned/persistent/general-dialog history parity.

Recent searches retains up to 20 valid queries successfully submitted to a worker, newest first. Exact query/mode/wrap duplicates move to the front; identical text with different encoding or wrap remains distinct. Invalid input, page navigation, no-work boundary requests and failed submission do not add entries. A queued query remains useful history even when its worker is later cancelled, finds no match or fails source validation; it is not a history of successful results.

Choosing an entry restores query/encoding/wrap and clears stale match/status/error while retaining the page and position history. It does not navigate, queue work or promote the entry until a later actual search is submitted. Query focus is requested after recall. Clear recent searches discards only the saved entries; current input/page/settings remain. The menu is disabled while busy or empty; private recall also refuses changes while work runs. Closing/replacing the viewer drops history. Reload retains recent queries for retry; nothing is written to settings or disk.

Saved raw query text is additionally capped at 16 KiB across all entries, pruning oldest entries first. A single query exceeding that budget is not saved and does not evict existing history; search semantics remain unchanged. UI text is already bounded by the existing 1024-character field. Menu labels escape control characters, truncate at a bounded width and provide full text on hover; recalled query uses original text, not its escaped label. A shared input-mode label identifies every supported mode.

Three regressions cover count/text caps, exact and mode/wrap-aware deduplication, oversized-history omission; actual valid/invalid worker submission and no-work recall/clear semantics; a headless egui pointer selection restoring KOI8-U query/wrap without changing page or queuing work. Existing encoding/keyboard/pointer/scanner tests remain in the full suite.

Fresh native validation: CUA became available again. Extended HexSearch now opens Recent searches, waits for popup placement, selects the newest query, then checks resumed backward/forward cyclic search through phase 84. Final capture passed **14/14 checks** with all native-effect counters zero. Binary hash and post-launch manifest timestamps were verified, and the final framebuffer was inspected. The short-lived QA app completed before AX observation (CUA returned AXError.cannotComplete); the fresh completed manifest/frame and terminal process state establish the capture.

The first native capture also passed generic checks but exposed a narrow vertical More encodings button. It is retained as `hex_search-initial-layout` and rejected for layout. Both legacy and recent menu triggers now allocate nonwrapping buttons directly in the parent wrapping layout, with popup content separate; this prevents nested enabled-scope width collapse. A headless single-line height assertion covers the legacy trigger. The corrected `hex_search/frame.png` shows readable controls and expected wrapped match at 110001. This qualification covers recent recall/resumed search and corrected trigger layout, not all older pending codepage/archive/keyboard/clipboard workflows.


Validation and rejected evidence:

- `test-initial.log`: 2794 passed / one failed pointer test; its click used a popup rect before viewport placement stabilized.
- `focused-initial.log` and `pointer-diagnostic.log`: retained reproductions/geometry diagnostics. The pointer test now waits for stable popup placement and still verifies exact recalled query/mode/wrap through real clicks.
- `focused-layout.log` and `pointer-backtrace.log`: temporary test instrumentation called Response.clicked inside Context.data_mut, causing a nested egui lock. Instrumentation was removed; this was test-only, not native production code.
- `clippy-layout-initial.log`: a test attempted the absent Context.style API; the default-context single-line height assertion now uses its explicit 30 px bound.
- `focused-final.log`: all three recent-search regressions passed after diagnostic cleanup.
- `test-verified.log`: final full suite **2795 passed, 3 ignored, zero failed**, including menu-height regression.
- `clippy-verified.log`, `check-verified.log`, `build-verified.log`: all-target visual-qa Clippy with warnings denied, default locked check and visual-qa build passed.
- Formatting/whitespace checks passed. `source-sha256.json` and `native-validation.json` bind the final source/binary and fresh native result. Earlier successful/intermediate logs remain separate.
