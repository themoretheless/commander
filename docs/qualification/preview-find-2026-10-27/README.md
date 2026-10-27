# Find inside previews — qualification, 2026-10-27

Independent implementation inspired by the find actions and case-sensitive
option in [Double Commander’s viewer](https://github.com/doublecmd/doublecmd/blob/master/src/fviewer.pas).
No upstream Pascal source was copied.

## Functional scope

Literal Unicode search, optional case sensitivity, non-overlapping matches,
wrapping next/previous navigation, match counter and current-match underline.
Enter/Shift+Enter navigate while the search field owns focus. Search indexes the
actual laid-out display text, including formatted Markdown/HTML, and does not
index HTML content hidden by the formatter. Table mode is excluded; CSV/TSV
Source mode supports search. Cmd+F still opens the existing file finder.

The query limit is 256 characters; at most 4096 matches are retained, with `+`
reported for additional matches. Text loading retains its existing 256 KiB limit.
No file reads, external tools or network requests are introduced. Match state is
cached per pane. Search reveals results immediately without scroll animation.
Changing content/query/path resets navigation, while reflow preserves it.

## Evidence

- Existing preview regression after integration: 69 passed, no failures
  (`preview-regression.log`).
- Final focused search checks: 6 passed, no failures (`search-tests.log`), including
  actual scrolling to a match after 200 lines, Enter/Shift+Enter, Unicode character
  offsets, literal metacharacters, case sensitivity, no matches, explicit match
  cap, wraparound, hidden HTML, reload invalidation and reflow preservation.
- Final Clippy across all targets/features with warnings denied: passed.
- Formatting and whitespace checks: passed.
- Native `code_preview` scenario now waits for the named cursor entry before
  opening the preview, then requires loaded text and exactly two indexed matches.
  Its real framebuffer capture is retained with manifest/capabilities. Visual
  inspection checks the current-match underline, readable navigation buttons,
  syntax colors and match count. Generic geometry/pixel checks are supplementary;
  they alone do not prove the search UI works.

The test filters overlap; counts must not be added as a unique-test total.
No complete VoiceOver, installed-release or large-dataset performance claim.

## Rejected evidence and corrections

Initial input simulation used an obsolete RawInput field and did not compile.
An early Clippy check rejected single-range array assertions; these assertions
were rewritten without suppressing the lint. Initial native attempts timed out
because the fixture attempted to open the preview before deferred cursor focus
completed. The harness now waits for the actual file binding. A captured draft
showed unsupported arrow glyphs on navigation buttons; these were replaced with
Prev/Next. A distant-result test initially failed with default scroll animation;
instant search navigation now passes it. Rejected logs/frame are retained under
`rejected-native` for traceability and are not accepted verification evidence.
