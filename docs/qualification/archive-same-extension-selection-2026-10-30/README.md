# Archive same-extension selection — 2026-10-30

Independent adaptation of [Double Commander MarkCurrentExtension / UnmarkCurrentExtension](https://doublecmd.github.io/doc/en/cmds.html#catmark), cohort #3.

The archive selection toolbar gains Same extension, with Select visible matches and Deselect visible matches. The focused row must be a visible regular archive file. Commands change only visible regular members matching its final filename suffix, using the same cached Unicode lowercase paths and borrowed extension-key helper as archive sorting. They neither navigate nor extract. Existing hidden/cross-folder selections and unmatched selections survive. Real directories, links, nonregular members and synthetic navigation folders are excluded; duplicate member paths remain separate IDs. No focused regular row disables the menu.

An empty suffix groups extensionless files, `.profile` and trailing-dot names; `.config.json` uses json, archive.tar.gz uses gz. This is final-suffix matching, not MIME detection or compound archive suffix detection. Scope follows current folder/query, and invalid/stale/absent cursor or mismatched cache produces no matches. Paths with display-lossy conversion retain existing archive display/filter semantics; no byte-exact Unicode-normalization parity is claimed.

Two regressions exercise actual folder/query caches, case variants and duplicate paths, hidden/unmatched selection retention, regular-only admission, hidden/stale/synthetic cursor and cache rejection, empty and compound suffixes, and Unicode case matching.

Native menu clicks, focused-row affordance and toolbar layout remain pending: desktop CUA is unavailable. Earlier archive native gates remain open; no native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2756 passed, 3 ignored, zero failed** (`test.log`).
- Initial Clippy rejected `pub(crate)` inside the private archive_order module; changed the shared helper to `pub`, matching module visibility conventions. Rejected output retained as `clippy-initial.log`; no behavior changed.
- Formatting and whitespace checks passed. Final Clippy/check/build logs accompany this report.
- Final all-target visual-qa Clippy with warnings denied, default locked check and visual-qa build: passed. Source/binary hashes and pending native-validation record accompany this report.
