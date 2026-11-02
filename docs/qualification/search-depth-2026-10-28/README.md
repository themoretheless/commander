# Search depth — 2026-10-28

Independent adaptation of fd minimum/maximum/exact depth controls, documented in [fd CLI source](https://github.com/sharkdp/fd/blob/master/src/cli.rs). The repository is in the researched cohort. No upstream code copied.

Find accepts `depth:<=1`, `depth:>=2`, exact `depth:2`, and ranges such as `depth:1..3`. Direct children are level one; the root itself is never emitted. Repeated maximum conditions use the smallest maximum. Invalid, negative, overflowing and reversed range values return query errors. Text expressions and saved JSON retain the conditions.

Live search passes the maximum into jwalk's traversal limit. Indexed search excludes deeper documents before emission. Both providers avoid opening an archive at the maximum level. Members of an in-scope archive are filtered using virtual path components, so `bundle.zip!/nested/arch.txt` is level three relative to the containing folder. A minimum only filters results and does not prune ancestors. Existing archive inspection and byte/member budgets remain in force: a depth limit is not a promise to avoid decompressing every deeper member of an archive already opened within the boundary.

Regression coverage checks grammar and persistence plus live/indexed agreement on root, nested and ZIP content hits for maximum, minimum, exact, range, zero and repeated-maximum queries. The live shallow query asserts exactly three scanned entries, proving that deeper filesystem entries and archive expansion are absent. It also checks out-of-root rejection and depth relative to an archive root. No native UI interaction or capture; no commit or push.

Validation: **2477 passed, 0 failed, 3 ignored** in the full locked suite. Both focused regressions passed. All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected test or Clippy run for this feature.
