# OR alternatives in pane-name filters — 2026-10-28

Independent adaptation of [fzf OR filtering](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). No upstream code copied. .rs$ | .toml$ selects either ending; alternatives can use existing fuzzy, literal, anchored or negated matching.

Only a standalone pipe separates branches (whitespace or a boundary on each side); embedded x|y remains ordinary fuzzy text. Branch edges are trimmed. An empty branch, including an unfinished trailing pipe, matches nothing rather than broadening the result. Leading apostrophe retains its established whole-input literal priority, including any pipes. This is OR across whole patterns, not fzf's full term/AND/parenthesis grammar.

The borrowed branch iterator adds no per-candidate strings to filtering. The first matching branch supplies highlight ranges; a matching negative branch has none. Facets still combine with the whole OR result, and excluded selections remain selected. The owning search provider recognizes composite filters rather than sending their raw text to its plain fuzzy scorer. Existing typed NameFilter export preserves expressions and JSON without a new schema.

Validation:

- Full local suite: **2464 passed, 0 failed, 3 ignored**. New matcher regression covers suffix unions, fuzzy alternatives, Unicode prefixes, negative alternatives, first-branch highlighting, literal/embedded pipes and incomplete/empty branch rejection.
- Listing regression verifies OR-specific result sets and warm-cache reuse, including partial and quoted expressions. Live-panel regression verifies facet composition and excluded-pick retention.
- Query regression roundtrips four composite/literal expressions through text and JSON, and compares the owning search matcher against the pane matcher across six candidate names per expression, including fuzzy and Unicode cases.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native OR-input interaction/frame capture or saved-search UI workflow was run. No commit or push.
