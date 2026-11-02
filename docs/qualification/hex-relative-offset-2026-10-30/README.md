# Hex relative offset navigation — 2026-10-30

Independent adaptation of [Far Manager ViewerGotoPos](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Verified upstream help documents signed relative decimal/hexadecimal/percentage positions. Commander retains its existing decimal/0x/whole-percent grammar and adds signed relative navigation; Far's alternate dollar/h/m notation and column positioning are not claimed.

Go to offset now accepts +64, -0x10 and signed whole percentages. The base is the loaded page's first byte, not the top rendered row, scroll position or an implicit cursor. A signed percentage computes a step from the full loaded file size using existing exact u128 percentage arithmetic, then adds/subtracts from the base. Unsigned decimal/0x/percent positions retain their absolute meaning. Leading plus is now explicitly relative (the old unsigned integer parser accepted a plus as absolute decimal syntax).

Relative input requires a loaded page; checked u64 addition/subtraction rejects overflow/underflow and a known-size check rejects destinations beyond EOF before queuing read work. Exact EOF and zero steps remain valid, including empty files. Missing magnitudes, repeated signs, whitespace immediately after a sign and malformed magnitudes fail. Existing outer trim and magnitude parsing remain. Failure sets the existing local error without discarding the loaded page. Success uses the existing deferred request owner and identity/cancellation/read budgets; it does not write files or change bookmarks. As with prior absolute navigation, a source replacement is still checked by the worker.

Two regressions cover positive/negative decimal/hex/percent values, exact rounding/bounds/EOF, u64 extremes, missing page, malformed signs and empty file; real deterministic worker movement across 4096-byte boundaries and percentage steps, resulting byte content, invalid-input no-op and no extra work. UI wiring was inspected but is not a headless/native pointer qualification.

Native Go/Enter entry, relative error state and layout remain pending because desktop CUA is unavailable. Historical hex captures do not qualify the new syntax/current binary; earlier native gates remain open. No native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2783 passed, 3 ignored, zero failed** (`test.log`).
- Formatting and whitespace checks passed (`fmt.log`, `diff-check.log`).
- All-target visual-qa Clippy with warnings denied, locked default check and visual-qa build passed (`clippy.log`, `check.log`, `build.log`). Source/binary hashes and pending native-validation record accompany this report.
