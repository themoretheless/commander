# Conjunctive pane-name filters — 2026-10-28

Follow-up to [fzf's multi-pattern filtering](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). Commander uses an explicit standalone && operator, rather than changing its existing space-containing fuzzy queries. This is an independent syntax adaptation; no upstream code copied.

.rs$ && !^test requires both a Rust suffix and the absence of the test prefix. AND binds more tightly than OR, so .rs$ && !^test | .toml$ also accepts any .toml ending. Each term uses the existing fuzzy/literal/anchored/negative matcher. Double-quoted terms protect internal operators; the existing leading-apostrophe whole-input escape remains unchanged. Embedded ampersands and ordinary spaces keep their previous fuzzy semantics. Parenthesized grouping is not introduced.

The shared borrowed scanner recognizes quoted terms after either operator, avoiding an internal pipe being misread as an alternative. Empty or malformed terms invalidate the whole expression. Filtering allocates no per-candidate term strings. Highlights combine positive term ranges and merge sorted overlaps/adjacent ranges in place; negative-only conditions have no highlight. The owning search adapter detects conjunctions and uses the same matcher instead of treating the whole expression as raw fuzzy text. Typed NameFilter export preserves expression/JSON without a schema change.

Validation:

- Full local suite: **2471 passed, 0 failed, 3 ignored**. New matcher regression covers AND/OR precedence, positive/negative conditions, merged overlap highlights, quoted pipes/ampersands, whole-input apostrophe escape, ordinary space matching and invalid/incomplete terms.
- Listing regression verifies expected row sets, precedence and warm-cache reuse. Live-panel regression verifies facet composition and excluded-selection retention.
- Query regression roundtrips four compound/unfinished expressions through text and JSON and compares the owning search matcher with the pane matcher over five candidates each. Existing quoted-OR and negation regressions passed after the shared scanner refactor.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native AND-input interaction/frame capture or saved-search UI workflow was run. No commit or push.
