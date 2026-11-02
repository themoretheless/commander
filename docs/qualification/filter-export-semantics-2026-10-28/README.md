# Preserve file-pane filter semantics in saved queries — 2026-10-28

Follow-up to the fzf-inspired literal and anchored file-pane filters. No upstream code copied. A reproduced defect showed ^i.txt$ matching İ.txt in the pane but not in its saved-query conversion: pane character folding and the generic regex backend used different Unicode equivalence rules. Converting ordinary fuzzy filters to generic exact NameContains also lost subsequence behavior.

Newly exported pane filters now use a NameFilter predicate, rendered as a filter: query term. It retains the normalized original query syntax and is evaluated by the same pane matching/highlight helpers. Ordinary fuzzy patterns keep their existing scorer; literal/anchor patterns keep their matching ranges and character fold. The predicate's behavior is independent of the overall query match mode. Metadata facets remain conjunctive. It uses the owning search provider's compiled name-matcher route; no content reads are added.

Query expressions quote the stored filter text so apostrophes, spaces, colons and backslashes survive parsing; JSON serialization retains the typed predicate. Existing name:/path:/content: queries and already-saved generic predicates keep their previous semantics. Older saved pane-derived queries are not guessed/migrated automatically; resaving from the pane records the new predicate. No actual desktop restart or saved-search UI interaction was run.

Validation:

- Reproduction before the fix: the new regression failed on ^i.txt$ against İ.txt (pane true, converted search false); log retained.
- Full local suite after the fix: **2457 passed, 0 failed, 3 ignored**. The new shared-kernel comparison covers six fuzzy/literal/anchored patterns × six Unicode/ASCII candidates, including dotted I and both Greek sigma forms. Existing live-listing/export regression also passed.
- New query regression roundtrips expression and JSON for eight patterns, including spaces, anchors, quotes, colon and backslash. It verifies filter: remains literal/anchored under overall Regex mode. Existing legacy predicate/parser/search tests passed.
- Final all-target Clippy with visual-qa and warnings denied passed. Initial Clippy requested backticks around a type name in a comment; corrected. Full suite predates only comment/documentation refinements. Formatting and scoped whitespace checks passed.

 No commit or push.
