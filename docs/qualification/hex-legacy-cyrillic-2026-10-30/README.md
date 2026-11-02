# Hex legacy Cyrillic search — 2026-10-30

Independent extension of [Far Manager CodePagesMenu](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37, applied to Commander's explicit exact-byte search input. Upstream groups system/Unicode/installed codepages; Commander offers a fixed portable subset rather than locale-dependent ANSI/OEM, autodetection or full installed-codepage parity.

More encodings adds KOI8-R, KOI8-U and IBM866. The menu label shows a selected legacy encoding and retains the other direct mode buttons. All five single-byte modes share existing encoding_rs conversion, pre-encoding character cap, 256 encoded-byte cap and error-before-return policy. Unrepresentable characters fail without replacement/transliteration. Whitespace, case and NUL are preserved; Unicode normalization and BOM injection are absent. The query is explicit Unicode text encoded to bytes, not whole-file decoding or encoding detection. KOI8-U retains its Ukrainian distinctions, and IBM866 means the fixed DOS codepage, not the host OEM setting.

A shared mode-change helper keeps direct buttons and the new menu consistent: mode changes clear stale match/search/error state, retain query/page/history, and are ignored while work is active. New modes reuse the same bounded exact-byte scanner, backward/forward/wrap behavior, progress/cancel and final file/page guards. No new dependency, filesystem mutation or clipboard action is added.

Known vectors were independently checked with Python's koi8-r/koi8-u/cp866 codecs: Cyrillic Привет/ё, Ukrainian і/ї/є/ґ case pairs and DOS Ё/ё. A parser regression checks those literal bytes, 256 single-byte characters despite larger UTF-8 input size, empty/over-limit/unrepresentable/combining queries, whitespace and NUL. Existing no-worker-on-error and cyclic worker regressions now include all three encodings, with cross-chunk hits and both wrapped directions. A headless egui pointer test selects KOI8-U and verifies stale-state reset without losing page/query/history or queuing work.

Native menu/layout and legacy search workflows remain pending because desktop CUA is unavailable. Historical hex captures do not qualify the new modes/current binary; earlier native gates remain open. No native capture success is claimed.

Validation:

- `test.log`: initial full suite, 2791 passed / 3 ignored, before adding the pointer-menu regression.
- `test-final.log`: final full suite including pointer menu, **2792 passed, 3 ignored, zero failed**.
- All-target visual-qa Clippy with warnings denied, locked default check, visual-qa build, formatting and whitespace checks passed (`clippy.log`, `check.log`, `build.log`, `fmt.log`, `diff-check.log`).

Source/binary hashes and pending native-validation record accompany this report.
