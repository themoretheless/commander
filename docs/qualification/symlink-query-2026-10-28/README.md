# Symbolic-link search — 2026-10-28

Independent adaptation of fd's [symbolic-link type filter](https://github.com/sharkdp/fd/blob/master/src/cli.rs). No upstream code copied; Commander retains conjunction semantics for repeated conditions rather than fd's multi-type union.

Find accepts `type:symlink` and `type:link`, in every text mode and in saved queries. The predicate selects symbolic links themselves, including dangling links, file links, directory links and loops. Hard links and ordinary files/directories do not qualify. Lexical link classification does not require resolving targets. This predicate does not alter the providers' existing traversal, content-read or archive-inspection policy; adding content predicates still requires available text under that provider's rules.

Live candidates carry the observed lexical kind. Index documents store an optional observed link flag; serialized snapshots preserve it without constructing artificial live filesystem identities or re-statting cached paths. Legacy records lacking the flag remain readable and have unknown kind: they cannot prove a link match, count toward the existing skipped/partial-coverage total, and need a rebuild for this filter. ZIP Unix link attributes and TAR symbolic-link header types are retained; TAR hard-link headers remain distinct. Synthetic archive parent folders are ordinary directories.

The query serialization test covers aliases, case-insensitive field names, every mode, conjunction and unknown synthetic metadata. A real filesystem/index/ZIP/TAR matrix covers live and cached searches in every mode, broken links, directory links, loops, hard links, archive enable/disable, and no directory-link recursion. JSON-round-tripped indexes retain kinds; deleting a link after indexing still returns the cached observation, proving lookup does not require live kind re-observation. Removing the kind field reproduces old snapshots and verifies partial-coverage accounting. Existing archive extraction/content policies remain independently tested by the full suite.

No new UI widget or native UI capture. No commit or push.

Validation: **2524 passed, 0 failed, 3 ignored** in the full locked visual-qa suite. Both focused regressions passed before final legacy skipped-count coverage was added; the full suite passed the expanded matrix, existing no-follow index checks and the maximum-metadata budget regression. All-target visual-qa Clippy with warnings denied, formatting and scoped whitespace checks passed. No rejected run for this feature.
