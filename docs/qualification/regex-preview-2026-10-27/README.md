# Regex search in text previews — 2026-10-27

Independent adaptation inspired by regular-expression search in
[Double Commander's built-in viewer](https://doublecmd.github.io/doc/en/viewer.html).
Uses the existing regex 1.13.1 dependency and its
[documented syntax](https://docs.rs/regex/1.13.1/regex/#syntax); no upstream
implementation code or new dependency was added.

## Behavior

**Regex** is an optional checkbox beside Match case and Prev/Next in Find in
preview. Literal search remains the default and continues to escape regex
metacharacters. Regex supports Unicode, groups, alternation, character classes,
repetition and inline flags. Unsupported look-around/backreferences or invalid
syntax produce an explicit error with Retry rather than a misleading no-match
count. The Match case checkbox sets the engine's default case option; standard
inline regex flags can override it.

Search operates on the displayed galley's text: plain/source previews,
formatted Markdown/static HTML and the current PDF page text use the shared
control. Hidden HTML/script content and unloaded PDF pages are not searched.
The existing Prev/Next and Enter/Shift+Enter navigation wraps among the indexed
matches. Only the current result is underlined and revealed; zero-width results
receive a minimum-width marker. Character ranges preserve Unicode cursor offsets
and text selection. Search does not modify, replace or save file content.

## Background work and bounds

Regex compilation and indexing run in an interactive job on the existing shared
workload scheduler. Query edits debounce for 150 ms; an unchanged query/text does
not recompile every frame. Layout-only reflow keeps the current result. Query,
mode, case, path or displayed-text changes discard the old receiver and cancel
its job before requesting new results. A late old completion cannot update the
new index. Job ownership is shared across the temporary UI-state clones.

The UI shows Searching, Cancel search, explicit errors and Retry. After both
panes render, jobs belonging to hidden or closed preview controls are retired;
returning to that unchanged view offers Retry. Retiring one pane does not cancel
the other. Completed jobs disarm cancel-on-drop; admission failure or an abandoned
worker becomes a terminal error without an indefinite spinner.

Queries are limited to 256 characters, regex input to 256 KiB of displayed UTF-8,
and indexed results to 4096, with a + indicator when more results exist. Regex
compilation and lazy DFA cache each have a 1 MiB engine budget; nesting is capped
at 32. These are component limits, not a measured whole-process RAM ceiling.
Oversized displayed input fails rather than silently searching a prefix.

Cancellation checks run before/after compilation and between match results.
A single regex engine call cannot be preempted, so there is no hard millisecond
cancellation guarantee. This distinction matters for greedy iterators whose
repeated searches can be expensive even without backtracking. Such calls stay
off the UI thread, under the existing shared scheduler's concurrency limits.
No speedup or timing benchmark is claimed.

## Verification

The full current suite passed: **2313 passed, 3 ignored**. It includes the six
existing preview-find tests and six new regex/worker tests: Unicode character
ranges, anchors and zero-width markers, captures, case, malformed/unsupported
patterns, query/input/program/nesting/result limits, cancellation, debouncing,
discarded old completions, normal completion accounting, hidden-view cleanup,
independent pane jobs, admission failure and abandoned-job Retry. Existing
source/document/PDF and app tests remained enabled. An initial Clippy run flagged
single-range test fixture notation; its log is retained under rejected/.
After rewriting those fixtures, Clippy with all targets/visual-qa and -D warnings
passed. Formatting and scoped Git whitespace checks passed.

The new native regex_preview scenario opens a real text file, starts with a
literal query that has no matches, clicks the Regex checkbox through pointer
input and waits for the background job to produce two case-insensitive matches.
The macOS eframe/Glow framebuffer passed **14/14** frame, glyph, layout, pane-hover
and native-effect-isolation checks and was visually inspected. No clipboard,
open or trash port was called. The screenshot shows item-12 and ITEM-34 and the
Regex toggle/count. This directly qualifies the plain-text fixture; the shared
formatted/PDF control is supported by code-path and headless regression evidence,
not a new complete native PDF/HTML matrix. The CI/README now list 18 scenarios;
no remote CI run or push is claimed.
