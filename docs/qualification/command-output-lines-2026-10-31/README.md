# Command output logical-line navigation — 2026-10-31

Independently adapts the line-number and Go to line workflow in [Double Commander's viewer](https://github.com/doublecmd/doublecmd/blob/master/src/fviewer.pas), cohort #3, to Commander's completed command output. It reuses Commander's existing PreviewLines implementation; no upstream implementation was copied.

**Line numbers** adds a separately painted gutter; it is off by default. Logical lines count newline delimiters plus the final line, including empty/trailing lines. Wrapping continuation rows do not receive new numbers. The count and **Go to line** input/button remain available with numbers off. Enter is consumed only while the number field owns focus and triggers navigation rather than Run or Find next. Invalid/zero/out-of-range/overflow input shows the valid range and does not request a new scroll.

Numbers address the combined displayed output, including the inserted newline before stderr, its heading, and retained truncation notices. They do not identify source-file lines or restore discarded process output. A successful empty capture has the one-line `(no output)` display. Copy stdout/stderr/text/JSON uses original CommandOutput; the gutter is never serialized or inserted into the selectable text.

The modal directly owns PreviewLines. Source changes reset its line-input/error/pending jump and index; wrap/width/palette changes update geometry while retaining the navigation state. Content and galley geometry are cached; only visible logical line numbers are painted. A new run resets view state; closing the modal releases it. The existing pane preview owner continues using load/save backed by egui context. The shared toolbar now consumes focused Enter before TextEdit to prevent it escaping to enclosing actions.

Verification logs/native artifacts are recorded alongside this file. Streaming output, persistent job history, source-file navigation, full accessibility/physical OS key qualification and huge-output performance remain open.

## Verification

**2999 tests passed, 3 ignored, zero failed**. All six checks exit 0: visual-qa build/full tests/all-target Clippy with warnings denied, default-feature check, formatting and diff whitespace. The new integration regression uses 250 Unicode/CRLF output lines plus the trailing line, jumps to logical line 200 using focused Enter, requires the actual scroll offset to exceed 2000 and the revealed row to be visible, then rejects 0 without moving the viewport or changing the exported report. Shared line-owner wrapping/empty/trailing/cache/invalid-input and existing output-find tests remain green.

Native **per_item_command** passed **16/16** capture checks; the framebuffer was inspected. After the existing real two-command/report-copy/search workflow, the scene clicks Line numbers, enters 9 into the actual line field and sends Enter. Readiness requires 10 logical lines, successfully revealed line 9 intersecting the real scroll clip, no error/no running command, and the unchanged 2/2-success batch. The capture visibly shows numbers 8, 9 and trailing 10, with `Design` at 9. The existing Regex search remains at match 1/2; Enter has not advanced it or restarted the command.

`native/per-item-state.json` records `[numbers_enabled, logical_count, revealed_line, error, visible]` as `[true,10,9,false,true]`. Independent artifact assertions also verify retained stream/report equality, original input/CWD context and byte-identical build/core/launch hashes. Source hashes are recorded. Effect counters remain exactly three recording clipboard writes and zero other effect-port calls; harmless shell execution is exercised separately. The fixture injects saved templates/opening context and does not write host templates, trust or clipboard.

Native coverage qualifies numbers-on and a focused Enter jump with wrapped output. The Go button, invalid/boundary/large jumps and numbers-off behavior use shared owner/model regressions rather than separate native captures. Full pane preview interaction, physical OS key delivery and complete VoiceOver behavior are not established by this scene.
