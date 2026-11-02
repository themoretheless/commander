# Archive selection memory — 2026-10-30

Independent adaptation of [Double Commander SaveSelection / RestoreSelection](https://doublecmd.github.io/doc/en/cmds.html#catmark), cohort #3. Upstream documents saving selected items in an internal buffer and reselecting them later.

Archive Save selection stores one explicit snapshot of all selected real member listing IDs, including filtered/cross-folder members. Restore selection replaces the current set with that snapshot; it does not merge. Saving again overwrites the previous snapshot. An explicitly saved empty set is distinct from no snapshot: Restore is enabled and clears the selection. Duplicate paths retain independent IDs. Save and restore admit only real member IDs, excluding synthetic navigation rows.

The snapshot belongs to MemberFilter and its immutable listing: filter/sort/folder/history changes retain it, while new listing publication, retry reset and closing the pane discard it. It stores at most the listing's 100,000 real IDs in one vector (at most 800,000 bytes of ID payload on this 64-bit target). It does not persist to settings, read/write clipboard or extract files. Cursor, folder, query and ordering are not restored. Restore clears the range anchor to avoid extending from an unrelated previous selection; it does not alter the worker's extraction snapshot.

Two regressions cover hidden and duplicate member identity, actual folder/filter/history changes and export order; absent versus empty snapshot, repeated restore, overwrite and listing-owner reset.

Native Save/Restore clicks and toolbar layout remain pending: desktop CUA is unavailable. Earlier archive native gates remain open; no native capture or gesture success is claimed.

Validation:

- Locked visual-qa full suite: **2753 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied: passed (`clippy.log`).
- Locked default check: passed (`check.log`).
- Formatting and diff whitespace checks: passed (`fmt.log`, `diff-check.log`).

Locked visual-qa build passed (`build.log`). Source/binary hashes and the pending native-validation record accompany this snapshot.
