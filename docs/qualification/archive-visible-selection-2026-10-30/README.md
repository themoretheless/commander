# Archive visible selection operations — 2026-10-30

Independent adaptation of [Double Commander invert/unmark commands](https://doublecmd.github.io/doc/en/cmds.html#catmark), cohort #3.

Implemented `Invert visible` and `Clear visible` in the archive selection toolbar. Both consume the same real-member set as Select all / Select visible files, after current folder and path filter are applied. Hidden and cross-folder selections remain unchanged. Duplicate paths retain independent listing IDs; synthetic navigation folders never enter extraction selection. Invert is disabled with no real rows; Clear visible is disabled when none of those rows are selected. Existing Clear retains its global behavior. The toolbar wraps at available width.

Two regressions exercise actual MemberFilter folder/query transitions, duplicate paths, repeated inversion, hidden selected members, export ordering, navigation-only folder root, invalid regex and clearing all visible real members.

Native clicks/layout remain pending: no desktop CUA tool is available. No native screenshot or extraction/clipboard side effect is claimed. The earlier archive export/filter/check/history native gates remain open.

Validation:

- `cargo test --locked --features visual-qa`: 2749 passed, 3 ignored, zero failed.
- `cargo clippy --locked --all-targets --features visual-qa -- -D warnings`: passed.
- Formatting and diff whitespace checks: passed.

Locked default check and visual-qa build: passed. Source and built binary hashes accompany this report.
