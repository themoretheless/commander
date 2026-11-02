# Negated pane-name filters — 2026-10-28

Independent adaptation of [fzf negated exact matching](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). No upstream code copied. !backup excludes a literal substring, !^temp excludes a prefix and !.tmp$ excludes a suffix. !^name$ excludes a whole name. This is a single-pattern grammar, not full boolean/OR term composition.

Negation complements the existing case-folded literal/anchor matcher. Leading apostrophe takes precedence, so '! searches an actual exclamation mark. A lone ! excludes every name, including an empty name; !^$ excludes only empty names. Negated results have no matched ranges, avoiding fabricated highlights for text that is absent. Existing fuzzy/literal/anchor forms and significant quoted spaces retain their semantics.

The original query string remains the cache key and the newly saved NameFilter predicate, so query conversion and the owning search backend reuse the same negation rule. Facets still combine with the text filter; excluded selections are retained. No clipboard/file mutation or new schema is introduced.

Validation:

- Full local suite: **2460 passed, 0 failed, 3 ignored**. Pure tests compare six negated patterns with their positive complements over nine ASCII/Unicode candidates, check empty and anchored cases and literal escaping, and verify no negative highlights.
- Listing regression covers mode transitions, prefix/suffix/quoted exclusions and warm-cache reuse. Live-panel regression covers excluded selection retention, saved-query backend agreement and facet composition. The shared Unicode/export comparison now includes three negative patterns (nine patterns × six candidates).
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

No native negation-input interaction/frame capture or saved-search UI workflow was run. No commit or push.
