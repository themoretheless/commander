# Hex plain-byte clipboard export — 2026-10-30

Independent extension of [Double Commander viewer copying](https://doublecmd.github.io/doc/en/viewer.html), cohort #3, to explicit loaded-page byte representations. Existing text selection and Copy page retain their behavior; this is a separate export action, not full synchronized byte/ASCII selection parity.

Copy bytes offers HEX bytes, Decimal bytes and UTF-8 text. HEX uses uppercase two-digit bytes, Decimal uses three-digit byte values, both space-separated without addresses, ASCII columns or trailing separators. These choices are independent of the current display representation. UTF-8 text is enabled only when the entire loaded page passes strict decoding, and preserves whitespace/control characters without replacement or normalization. A page split inside a multibyte character disables this text action, even if the full file is valid UTF-8.

Copies only the loaded page snapshot, up to 4096 bytes, through the existing show-result/clipboard owner. No extra file read or freshness claim is made after loading; the snapshot may differ from a subsequently modified/deleted source. Controls are disabled while busy or without nonempty loaded data. Existing Copy page still emits its formatted offsets/values/ASCII dump. Hex/decimal output is bounded to 12287/16383 bytes per full page; UTF-8 output is at most 4096 bytes. This is page export, not arbitrary whole-file or selected-range export. Pasting more than 256 bytes into search remains subject to its separate pattern limit.

Regressions cover all byte values across a full page and exact round trips, output size/separators, invalid UTF-8, a nonzero loaded page with Unicode/controls after deleting its source, no queued reread, and a headless egui pointer menu action returning exact HEX output without file changes. Headless pointer coverage does not qualify native clipboard behavior.

Native menu/layout and clipboard formats remain pending because desktop CUA is unavailable. Historical hex captures do not qualify these new actions/current binary; earlier codepage/archive native gates remain open. No native capture success is claimed.

Validation:

- `test.log`: initial full suite (2776 passed / 3 ignored), before pointer-menu regression was added.
- `test-final.log`: final full suite including headless pointer action, **2777 passed, 3 ignored, zero failed**.
- Final source checks are recorded in `clippy-final.log`, `check-final.log`, `build-final.log`; initial successful checks remain separately retained.
- Formatting and whitespace checks passed (`fmt.log`, `diff-check.log`).

Final all-target visual-qa Clippy with warnings denied, locked default check and visual-qa build passed. Source/binary hashes and pending native-validation record accompany this report.
