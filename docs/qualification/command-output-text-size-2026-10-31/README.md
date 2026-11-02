# Command output text size — 2026-10-31

Independently adapts [Double Commander viewer font-size controls](https://doublecmd.github.io/doc/en/configuration.html#ConfigFonts), cohort #3, to Commander's completed command output. The existing preview typography toolbar is reused with caller-owned state; no upstream code is copied.

**Text size** offers **A−**, **A+**, and **100%**, changing in 25% steps between 50% and 200%. Only retained output text and its line-number gutter scale. Application chrome, input fields, pane preview typography, command metadata and report text remain independent. The setting is owned by the modal result view; a new run/window defaults to 100%, with no persistent preference or egui context value retained for the command toolbar.

Layout-cache identity includes text magnification. Wrapping and line geometry are recomputed at the selected size; logical-line counts, search offsets/query/options, Go input and retained streams stay unchanged. Font-dependent gutter measurement and drawing use the same scaled monospace size as the output, preventing large numbers from overrunning their reserved width. Existing pane previews continue through the original typography/line APIs and default font sizes.

This is text magnification, not terminal emulation, source editing, live output, Ctrl+wheel binding or persistent Jobs history. Exact captures and verification logs are recorded alongside this file.

## Verification

**3000 tests passed, 3 ignored, zero failed**. Full visual-qa tests include a new integration regression requiring increased real galley row height at 200%, a changed layout allocation with identical displayed text, unchanged Unicode match count/current match and logical line count, exact report equality after enlargement/reset, a fresh view at 100%, and no command typography preference retained in context memory. Existing shared typography pointer regressions cover lower/upper clamps, reset and independent pane state. Existing line/search/lifetime regressions remain green.

Native **per_item_command** passed **16/16** capture checks; the framebuffer was inspected. After real two-command execution, immutable report copying, Regex search and Go to line, the scene pointer-clicks A+ twice (125%, 150%), A− (125%), 100% (reset), and A+ (125%). Each next stage requires the preceding actual scale value. It then clicks the real Go to line button and requires original line 9 to be visible at 125%, still 10 logical lines, Regex ready at match 1/2, no running command and the original 2/2-success batch. The capture shows larger output and gutter text with stable control/chrome sizes.

Independent assertions over the copied artifacts require `text_percent=125`, visible line 9/no error, original streams, original command/CWD/input snapshots, exact attempted expansions and matching binary/core/launch hashes; qualified source hashes are recorded. Native effect counters remain exactly three recording clipboard writes and zero other effect-port calls; harmless shell execution is tested separately. Host templates/trust/clipboard are not written by the fixture. Physical OS key delivery, all extreme-size native layouts and full accessibility navigation remain outside this capture.

The first green capture/check run, before removing a default-build warning for the QA-only toolbar ID parameter, is preserved in `before-default-warning-cleanup/`. Final artifacts above were regenerated from the warning-cleaned build.

Visual-qa build, full tests, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and diff whitespace checks all exit 0; see `checks.json` and logs.
