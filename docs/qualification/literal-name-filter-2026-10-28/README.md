# Literal file-pane name filter — 2026-10-28

Independent adaptation of [fzf apostrophe-prefixed non-fuzzy matching](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). No upstream code copied. Commander interprets a leading apostrophe in a pane filter as one literal substring comprising all following text. This is not fzf's full extended term/OR/negation grammar.

Ordinary queries retain existing fuzzy matching. Literal queries retain spaces after the marker, including trailing spaces; punctuation and wildcards are plain characters. Case matching reuses Commander's one-character Unicode fold. A doubled initial apostrophe searches an apostrophe; a lone marker matches everything. Leading whitespace before the marker is ignored. The query is transient per-tab/folder view state like existing filter text, not a new session-persisted preference.

The listing cache and list-row character-range highlights share the new matching helper. Literal matching borrows character iterators without allocating per-candidate key strings; highlighting allocates its existing range vector. Facets still combine with the text filter. Conversion to a saved-search query removes the marker, preserves the literal text and uses the existing exact NameContains predicate. The filter tooltip explains the syntax; palette/omnibar fuzzy matching is unchanged.

Validation:

- Full local suite: **2452 passed, 0 failed, 3 ignored**. Pure matching tests cover fuzzy/literal separation, spaces including trailing spaces, punctuation/wildcards, apostrophes, empty literal queries and Unicode character-index highlight ranges.
- Listing regression verifies cache transitions between fuzzy/literal queries, significant trailing spaces and warm snapshot reuse. Panel regression verifies actual filtered rows, facet composition and preservation of excluded selections. Query-conversion regression verifies marker removal and trailing-space retention.
- Final all-target Clippy with visual-qa and warnings denied passed. Initial Clippy requested pub rather than redundant pub(crate) inside the private fuzzy module; visibility spelling corrected with no effective external API or matching behavior change. Full suite predates only this visibility-style correction. Rejected evidence retained.
- Formatting and scoped whitespace checks passed.

 No native literal-input interaction/frame capture or actual saved-search UI workflow was run for this addition. No commit or push.
