# Extension alternatives — 2026-10-28

Independent adaptation of [fd multiple-extension filtering](https://github.com/sharkdp/fd/blob/master/src/cli.rs) from the researched cohort. No upstream code copied; Commander uses its own query syntax.

Find accepts `exts:rs,toml,tar.gz`: OR inside each list, AND between lists and other conditions. Optional leading dots and quoted lists with spaces are accepted. Matching is files-only, uses a dot boundary, supports compound suffixes, defaults to case-insensitive and honors `case:sensitive`. Extension matching remains literal in Exact/Fuzzy/Regex modes. Expressions, chips and saved JSON preserve the list. Empty members, wildcards and path separators are rejected, including invalid direct deserialization inputs when validated.

Existing `ext:` retains its single literal suffix behavior, including comma-containing literal extensions. No migration or reinterpretation of saved queries. Normalized alternatives are prepared once when compiling the query.

Two regression tests cover serialization across all modes, literal boundaries, compound extensions, dotfiles, directory rejection, case, invalid lists, legacy comma suffixes, and agreement between live/indexed/ZIP searches combined with content conditions. Repeated lists intersect correctly; metadata rejection avoids unnecessary native/indexed binary content inspection.

Validation: **2518 passed, 0 failed, 3 ignored** in `cargo test --locked --features visual-qa`; both focused regressions passed. `cargo clippy --locked --all-targets --features visual-qa -- -D warnings`, formatting and scoped whitespace checks passed. No rejected run. No native UI capture for this query-language change. No commit or push.
