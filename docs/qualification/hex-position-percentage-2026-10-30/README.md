# Hex loaded-page position percentage — 2026-10-30

Independent adaptation of [Double Commander viewer status position in bytes and percentages](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Hex preview status now includes Position alongside its existing hexadecimal/decimal offset, loaded byte count and file size.

The percentage describes the loaded page's first byte relative to its bound file snapshot. It is rounded down to 0.01% using u128 multiplication/division over the u64 offset and size; no floating conversion is involved. Non-EOF positions never round up to 100.00%, including u64::MAX-sized snapshots. EOF is exactly 100.00%; an empty file shows n/a because its percentage is undefined. An out-of-range input to the formatter is bounded to 100%, while the existing read owner continues to reject invalid requested offsets.

The label and hover text explicitly distinguish position from background read/search progress or a live file-size monitor. Page reads, rendering layout, Copy page, byte-copy formats, bookmarks, query settings and position history are unchanged. The string is exposed through the existing test/visual-QA probe mechanism for later native validation; it does not trigger worker activity.

Two regressions cover exact fractional values, zero/EOF, near-zero/near-EOF rounding, u64::MAX half/last-byte cases and an actual deterministic read at offset 3 in a six-byte file. Rendering then produces the exact 50.00% status without queuing work or moving history. All ten focused position tests pass, including existing navigation/history coverage. Native status visibility/wrapping and EOF/empty-file presentation remain pending because CUA is not exposed in this continuation. No native launch was attempted and earlier viewer captures do not qualify the changed label.

Validation: locked visual-QA suite passed 2833 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
