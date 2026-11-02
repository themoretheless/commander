# Hex mixed literal/byte search — 2026-10-30

Independent adaptation of [Midnight Commander mixed quoted-text/hex viewer search](https://source.midnight-commander.org/man/mc.html), cohort #40. The manual documents mixing quoted text with hexadecimal constants, including optional 0x and leading zeroes. Commander independently implements a strict UTF-8-literal grammar and adds explicit supported escapes; complete MC locale/search grammar parity is not claimed.

Mixed text/hex is an additional search mode, leaving HEX's complete-pair syntax and other encodings unchanged. A query such as `"Header" 00 FF` encodes quoted text as exact UTF-8 and unquoted constants as hexadecimal bytes. Constants are hexadecimal even without 0x, accept uppercase prefixes/digits and leading zeroes, and must fit 00–FF. Tokens require ASCII whitespace separation. Literal whitespace, Unicode, NUL and Unicode scalar values are preserved. Quoted escapes support quote, backslash, newline, carriage return, tab and NUL; unknown/dangling escapes and unclosed quotes fail. Empty literals may occur beside real bytes but an all-empty pattern fails.

Pure parsing rejects input above 1024 Unicode characters before token construction and patterns above 256 encoded bytes during append. Unrepresentable syntax, out-of-byte-range constants, incomplete input and garbage return no partial pattern. No BOM or normalization is injected. The UI's existing 1024-character limit matches this parser. Mixed patterns reuse the same exact-byte worker and forward/backward/wrap/cancel/progress/file-page guards; input errors return before queuing work.

Two new parser regressions cover the upstream mixed example, UTF-8/controls/escapes, numeric hex/leading-zero semantics, strict token boundaries, malformed/empty input and both input/output budgets. Existing deterministic-worker cyclic-search coverage now includes a mixed Unicode/NUL/FF pattern across a 64 KiB boundary in both wrapped directions. Full existing scanner and UI tests remain included.

Native mode-button layout and mixed query workflow remain pending because desktop CUA is unavailable. Historical HexSearch captures do not qualify the new mode/current binary; earlier native gates remain open. No native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2781 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied passed (`clippy.log`).
- Formatting and whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked default check and visual-qa build passed (`check.log`, `build.log`). Source/binary hashes and pending native-validation record accompany this report.
