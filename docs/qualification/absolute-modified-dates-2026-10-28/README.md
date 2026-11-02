# Absolute modification-time bounds — 2026-10-28

Independent adaptation of fd's [changed-after/changed-before absolute-time filters](https://github.com/sharkdp/fd/blob/master/src/cli.rs), from the 99-project cohort. No upstream code copied. Commander syntax and timezone defaults are its own, not fd CLI parity.

Find accepts `after:2026-10-01 before:2026-11-01` and explicit RFC 3339 instants such as `after:2026-10-28T12:30:00+04:00`. Both bounds are strict. Calendar dates mean midnight UTC, independent of machine timezone; timed values require an explicit offset. Fractional seconds and pre-epoch instants are supported. Leap seconds and invalid dates are rejected. Missing modification metadata fails either bound. Contradictory ranges simply return no matches.

Conditions combine with all existing predicates through AND and survive expression/JSON round trips in every text mode. Existing relative date conditions are unchanged. Timestamps are validated and parsed while preparing the query; candidate evaluation compares compiled instants without reparsing date strings. Match explanations include the applied bound without fabricated score/highlight ranges. Live/indexed/archive candidates use their existing modification metadata; indexed timestamps reflect the cached snapshot, and archive precision/timezone limitations remain those of the existing provider.

Regression coverage exercises timezone equivalence, one-nanosecond strict boundaries, dates before 1970, fractional instants, invalid dates/leap seconds/missing offsets, unknown metadata, case-insensitive field names, direct saved-query validation and round trips. A real filesystem/index/ZIP matrix in Exact/Fuzzy/Regex modes verifies dates fixed in fixtures, conjunctions with extension/content conditions, contradictory ranges and explanations.

No native UI capture was performed for this query-language change. No commit or push.

Validation: **2520 passed, 0 failed, 3 ignored** in the full locked visual-qa suite; both focused regressions passed. All-target visual-qa Clippy with warnings denied, formatting and scoped whitespace checks passed. The first Clippy attempt rejected an unquoted `SystemTime` identifier in a doc comment; the comment was corrected and the rejected log retained.

Follow-up: the original provider matrix used whole-second dates and did not qualify indexed subsecond/pre-epoch precision. A later regression reproduced truncation in the existing index format; see [precise indexed timestamps](../precise-index-time-2026-10-28/README.md) for the correction, legacy-cache limits and retained failing evidence.
