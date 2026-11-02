# Configurable hex row width — 2026-10-30

Independent adaptation of [Far Manager adjustable hex bytes per row](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4) (`Viewer`, Alt+Left/Right), cohort #37. Commander exposes a Bytes per row integer control (1–64) and one-click 8/16/32 presets. No new Far keyboard bindings are claimed.

The default remains 16. Displayed and Copy page dump rows share the same layout renderer in HEX and Decimal, preserving absolute byte offsets, printable ASCII and padding of the final partial row. Wider rows separate every eight bytes. A bounded RowWidth owner keeps zero/out-of-range values out of row count/division/rendering. Width changes reset the inner byte-scroll view using its identity, so a stale row offset is not reused after reflow. Search-hit row highlighting uses the selected width.

Width is presentation state for the open viewer, survives Reload, and resets to 16 in a new viewer. Changing it does not queue work or modify the page, position history, queries or bookmarks. Reads remain bounded to 4 KiB per page, and plain Copy bytes exports are unaffected.

Two regressions cover all 64 widths and both radices, independently decode dump tokens back into every source byte from a non-aligned start, verify exact row offsets, ASCII and final-row column alignment, and cover empty input/bounds. An egui pointer regression clicks the 8-byte preset and Copy page, checks the exact dump against a golden string, no queued read, retained state and width retention across Reload. Direct native numeric entry, preset/copy interaction, horizontal layout and highlighting remain pending because CUA is not exposed in this continuation. No native launch was attempted; earlier viewer captures do not qualify the changed header/layout.

Validation: the locked visual-QA suite passed 2813 tests (3 ignored). Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
