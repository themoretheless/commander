# Streaming exact-byte search in Hex preview — 2026-10-27

Independent implementation inspired by hexadecimal, next and previous search in
[Double Commander's built-in viewer](https://doublecmd.github.io/doc/en/viewer.html).
No upstream implementation code or new dependencies are included.

## Behavior

The existing **View as hex** window now has a **Hex bytes** field, **Find next**
and **Find previous**. Enter complete hexadecimal byte pairs with or without
ASCII whitespace, in either letter case (`DE AD BE EF` or `deadbeef`). Patterns
are 1–256 bytes. Invalid characters, incomplete pairs, empty patterns and
oversized patterns produce explicit errors; no regex or wildcard interpretation
is applied.

The first search begins at the currently displayed byte offset, inclusive.
Later searches of the same pattern begin one byte after/before the last match,
so overlapping matches are supported. A new pattern starts at the current page.
Navigation/Reload resets the previous match anchor. Search does not wrap: reaching
the end/beginning reports no match in that direction. Use First or a byte offset
to choose another starting point. A failed search retains the current page.

A match opens a 4 KiB page beginning at its exact byte offset and highlights rows
containing the matched bytes. The match status gives that absolute offset.
Highlighting is row-level rather than linked hex/ASCII byte selection. Copy page
continues to copy the bounded page representation. No file editing or source
rewriting is introduced.

Search runs in a shared interactive Search job. Only one read/search job can be
pending for this viewer. While searching, the UI shows bytes read and provides
Cancel; navigation, copying and query changes are disabled until completion.
Cancel/close drops the task receiver, so a queued completion cannot replace a
new page. Cancellation clears the displayed page and offers Reload, consistently
with the existing page-read cancellation behavior.

## Scan and freshness boundaries

The scanner seeks/reads at most 64 KiB at a time and uses a prefix matcher whose
state survives chunk boundaries. Backward search scans each block in reverse
with a reversed pattern, including enough right context to recognize a match
starting exactly at the inclusive bound. Both directions find matches crossing
4 KiB preview pages and 64 KiB read blocks. No whole-file buffer or list of all
matches is allocated. The matcher table/pattern are bounded by 256 bytes.

The source size is captured at start; an append tail cannot extend the scan
indefinitely. Short reads and I/O errors are explicit failures. Interruptions
retry. Cancellation is checked between reads and every 4096 processed bytes,
and the existing I/O budget checkpoint is respected between blocks. The progress
counter records bytes physically read (which may include bytes beyond a found
match in that block), not a benchmark or percentage-complete estimate. Blocked
OS calls cannot be preempted.

Search binds to the viewer's prior regular-file stamp and reuses the same
no-follow open and before/after descriptor/path metadata checks as page reads.
Even a no-match result requires final validation. A found match is then read as
a page bound to that same stamp. File replacement or detected changes discard
both match and no-match results. The existing Unix ctime/identity checks and
non-Unix limitations apply; this is not an atomic filesystem snapshot and cannot
exclude undetectable concurrent writes. Source text decoding is not involved.

## Verification

Tests compare both directions with an independent naive window-search oracle
on varied binary fixtures. Other cases cover overlaps, repeated-prefix patterns
up to 256 bytes, empty/short files, EOF and u64 bounds, matches across page/read
boundaries, mixed-case/whitespace parsing, invalid patterns, interrupted and
fragmented reads, cancellation during scanning, file replacement during both
matching and no-match scans, and search near a sparse-file offset beyond 4 GiB.
Deterministic workload tests verify next/previous anchors, retained pages on no
match, cancellation accounting and discarded late search completions. Existing
page-reader and workspace tests remain enabled.

The native `hex_search` scenario creates a real binary fixture with matches at
4094 and 65534, invokes View as hex, enters lowercase spaced bytes, clicks Find
next twice and Find previous once. Capture requires the final match at 4094,
not merely the same page selected through Go. The macOS eframe/Glow framebuffer
was visually inspected for the query, status, actual bytes and row highlight.
The CI matrix now includes 17 cases; no remote CI run is claimed.

Final results: full suite **2307 passed, 3 ignored**. After the full run,
visual inspection led to using the theme's strong foreground for highlighted
rows. Following this display-only refinement, the **22 tests selected by the
`hex_` filter** passed again (including existing checksum-hex cases), Clippy with
all targets/visual-qa and `-D warnings` passed, and the native capture passed
**14/14** framebuffer checks again. The full suite was not repeated. Formatting
and scoped diff checks also passed. No commit, push or remote CI is claimed.
