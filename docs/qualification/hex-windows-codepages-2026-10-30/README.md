# Hex Windows codepage search — 2026-10-30

Independent extension of [Double Commander viewer encoding selection](https://doublecmd.github.io/doc/en/viewer.html), cohort #3, applied to Commander's exact byte-search input. Upstream documents encoding selection and viewer Find; this does not claim identical upstream search menus or complete encoding parity.

The hex viewer gains Windows-1251 and Windows-1252 search modes. Unicode query text is encoded with the existing encoding_rs dependency, then passed to the same bounded exact-byte search worker used by HEX/UTF-8/UTF-16. Input preserves whitespace, case and NUL. No BOM, normalization, transliteration or silent replacement is introduced. Empty queries, unrepresentable characters and encoded patterns above 256 bytes fail before queuing work. The encoder's replacement/error flag is checked before any bytes are returned; a Unicode-character guard bounds input encoding allocation as well. These are explicit Windows codepages, not locale-dependent ANSI/OEM or strict ISO-8859-1 aliases.

Forward, backward, overlap, arbitrary byte offsets, chunk boundaries, progress, cancellation, wrap and final file/page identity checks retain their existing scanner/worker owners. Changing mode clears stale match/search status as before. The viewer does not decode the whole file or infer its encoding. Existing HEX default and Unicode modes retain behavior.

One parser regression verifies known Cyrillic/Western/Euro/smart-quote byte sequences, spaces/NUL, exact cap, empty/unrepresentable/combining/BOM rejection. The existing deterministic-worker cyclic-search regression now includes both codepages and matches straddling a 64 KiB boundary in both wrapped directions. Another worker regression verifies unrepresentable input queues no search and retains loaded page bytes. Existing scanner cancellation/replacement tests remain part of the full suite.

Native mode-button layout and codepage search workflow remain pending because desktop CUA is unavailable. Historical HexSearch captures cover prior modes, not these new buttons or current binary. Earlier archive native gates remain open. No native capture success is claimed.

Validation logs:

- `test.log`: initial full suite (2773 passed / 3 ignored), before adding the no-worker-on-error regression.
- `test-final.log`: full rerun including the additional worker regression: **2774 passed, 3 ignored, zero failed**.
- `clippy.log`, `check.log`, `build.log`: all-target visual-qa Clippy with warnings denied, default locked check and visual-qa build passed.
- `fmt.log`, `diff-check.log`: formatting and whitespace checks passed.

Source/binary hashes and pending native-validation record accompany this report.
