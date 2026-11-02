# Archive extension alternatives — 2026-10-30

Independent adaptation of [fd repeated extension filters](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. A dedicated Extensions field accepts space-separated literal suffix alternatives such as pdf jpg tar.gz, optionally preceded by one dot. Multi-part suffixes match at a dot boundary on the final filename; parent names and suffix substrings do not match. Dotfiles such as .env match env. Comparison uses the listing owner's cached Unicode-lowercase names and is always case-insensitive, independently of path-query case/scope/regex options.

The parser is cached separately from path queries, bounded to 256 input bytes and 32 tokens, and rejects wildcard/path/list punctuation, empty dotted components and control characters inside tokens. Tokens are normalized, sorted and deduplicated. An empty field disables the constraint. Any alternative can match; the result intersects all other predicates and applies to all real member types, as fd documents. Valid navigation folders stay available; invalid input hides all rows and reports an error. Synthetic navigation IDs never become selected members. No member I/O is added.

Settings participate in full saved-snapshot identity, validity checks, 16 KiB history cap, recall, individual removal labels and Reset. Hidden selections and navigation/sort owners stay current. The stable field ID and accessibility label support subsequent native validation.

Two regressions exercise literal/multi-part/case/dotfile semantics, malformed/overlong/token-count failures, duplicate members, types, path inclusion/exclusion, valid/invalid navigation behavior, hidden selected copy, distinct saved identity, recall and Reset. The initial focused membership test expected listing order although the owner sorts by filename; its failed log is retained, and the corrected membership assertion sorts result IDs without changing product sorting.

The first full run also caught an overly broad test-text replacement that had changed a pre-existing selection assertion. That assertion was restored, the selected-copy check was scoped to the new regression, and the failed full log is retained.

Native field entry/error/recall/focus/layout gates remain pending because CUA is not exposed in this continuation. No native launch was attempted; older captures do not qualify the changed controls. No commit or push performed.

Final validation: locked visual-QA suite passed 2838 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source and binary hashes accompany the logs. Native validation remains pending.
