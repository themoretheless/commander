# Literal quoted branches inside OR filters — 2026-10-28

Usability follow-up to the [fzf-inspired OR filter transfer](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). Double-quoted whole-branch literals are an independent Commander extension, not a claim of fzf syntax parity. No upstream code copied.

The previous leading-apostrophe form deliberately made the whole input literal, so it could not express a literal pipe-containing first alternative followed by another alternative. Now "a | b" | .rs$ matches literal a | b text or the .rs suffix. Quoted branches preserve their inside spaces and operator characters. The existing apostrophe whole-input form remains available, including for literal double quotes. Backslash/embedded-quote escaping inside a double-quoted branch is not introduced.

The borrowed separator scanner ignores standalone pipes inside a quoted branch. A branch beginning with a double quote must have one matching closing quote at its end; malformed or unfinished branches invalidate the expression instead of broadening it. Embedded quotes in an unquoted pattern remain ordinary characters. Search dispatch recognizes quoted forms as structured filters rather than raw fuzzy strings. Existing shared matching/highlight/export routes carry the new behavior without a schema change.

Validation:

- Full local suite: **2467 passed, 0 failed, 3 ignored**. Matcher regression covers quoted pipe-containing first/later alternatives, significant spaces, literal anchors, highlight ranges, incomplete/malformed quotes, apostrophe escape and embedded ordinary quotes.
- Listing regression verifies quoted delimiter handling, expected row sets, incomplete input and warm cache reuse. Query regression roundtrips four quoted/unfinished expressions through text and JSON and compares the owning search matcher to the pane matcher across five candidates each.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native quoted-input interaction/frame capture or saved-search UI workflow was run. No commit or push.
