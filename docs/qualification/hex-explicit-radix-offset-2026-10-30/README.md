# Hex explicit radix offset forms — 2026-10-30

Independent adaptation of [Far Manager ViewerGotoPos radix notation](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Verified upstream help documents dollar/0x prefix, h suffix and m decimal suffix. Commander extends its exact byte-offset grammar without adding the upstream Hex value toggle, columns or full expression parsing.

Go accepts dollar-prefixed and h-suffixed hexadecimal byte positions, plus m-suffixed decimal byte positions. Existing 0x hexadecimal and unadorned decimal remain available; suffix/prefix letters are case-insensitive. Redundant matching hex markers (0xFFh or $FFh) are allowed. The m suffix means decimal notation, not a size unit. All forms integrate with existing leading-sign relative navigation, e.g. -$10 and +16m. Bare FF stays invalid; unadorned 10 is decimal while 10h is hexadecimal. Whole percentages retain their existing decimal-only grammar; radix-suffixed percentages are rejected.

Explicit digits are validated before conversion, rejecting internal signs, conflicting m/hex markers, missing digits, embedded whitespace and garbage. Checked u64 parsing rejects overflow. Relative addition/subtraction and known-file bounds retain existing checks; absolute beyond-EOF values retain worker validation. Values resolve to exact byte offsets, not row-aligned addresses or display-mode-dependent bases. No new reads/mutations occur during parsing, and successful navigation uses existing identity/cancel/read owners.

Two regressions cover equivalent forms, default-base distinction, u64 extrema/overflow, matching/conflicting markers, internal signs, relative integration, percentage rejection and actual deterministic page bytes across 4096 boundaries. Invalid relative targets retain the loaded page and queue no work. UI wiring is inspected; no headless/native pointer qualification is claimed for new entry syntax.

Native Go/Enter/radix entry and hover/layout validation remain pending because desktop CUA is unavailable. Historical hex captures do not qualify this syntax/current binary; earlier native gates remain open. No native capture success is claimed.

Validation:

- `test.log`: initial full suite, 2785 passed / 3 ignored.
- `test-final.log`: final rerun after explicitly rejecting internal signs in radix-marked numbers: **2785 passed, 3 ignored, zero failed**.
- All-target visual-qa Clippy with warnings denied and locked default check passed (`clippy.log`, `check.log`).
- Formatting and whitespace checks passed (`fmt.log`, `diff-check.log`).

Locked visual-qa build passed (`build.log`). Source/binary hashes and pending native-validation record accompany this report.
