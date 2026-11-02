# Remove individual hex input-history entries — 2026-10-30

Independent adaptation of [Far Manager individual dialog-history deletion](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4) (`MiscCmd`, Shift+Del), cohort #37 in the 99-project research. Recent searches and Recent offsets now have a Remove button beside each entry; this transfer does not add Far's shortcut or item locking.

Search deletion compares the complete query/encoding/wrap snapshot. Offset deletion compares exact expression text, so numeric aliases such as 0x10 and 16 remain distinct. Both remove only the targeted entry, preserving the remaining order and existing memory caps. Removal never invokes recall or Go/search. Active page, fields, mode/wrap, last match/status/error, position history and bookmarks are retained. Removing the last entry disables its opener; whole-history Clear remains available.

Both popups use a shared bounded row renderer with distinct recall and Remove hit targets, escaped labels and full hover details. Open popup items and Clear are disabled while a worker is busy, and mutation/recall callbacks also check busy state. This keeps popup actions under the same existing worker admission rule.

Two new pointer regressions drive the actual viewer menus with deterministic workloads, proving exact removal (including same-query different codepages/wrap, and equivalent but textually distinct offsets), no queued work, no recall, and retained page/input/search/bookmark/position-history state. All 14 pointer tests pass, including existing search/offset recall, actual Go, codepage selection, byte-copy and archive saved-filter actions.

Native new rows, Remove/recall interaction and popup layout remain pending. CUA is not exposed in this continuation, no native launch was attempted, and earlier HexSearch/HexPreview captures do not qualify these changed rows. No OS clipboard claim is made.

Validation: the locked visual-QA suite passed 2809 tests (3 ignored). Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
