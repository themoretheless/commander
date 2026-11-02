# Archive selection narrowing — 2026-10-30

Independent adaptation of [ranger narrow](https://github.com/ranger/ranger/blob/master/ranger/config/commands.py), cohort #30. Verified upstream command documentation/implementation snapshots the current selected basenames and disables narrowing when no items are selected. Commander independently uses immutable archive member IDs, retaining duplicate identities.

Narrow to selection snapshots all selected real member IDs and switches to All members so selections from multiple folders can be reviewed together. The current path filter and sorting remain active. Unselected snapshot rows remain visible until narrowing is refreshed, allowing a reviewer to deselect and reselect mistakes. Repeating Narrow replaces the snapshot. Empty (or all-invalid) selection removes narrowing; Remove narrowing does so explicitly without changing selection. Selected member actions retain their archive-wide scope, including members outside the snapshot; the extraction label counts those as hidden.

Folder navigation can further restrict the snapshot, and breadcrumbs/Up/history retain their existing owners. Synthetic navigation folders are omitted while narrowing is active. Real directory entries remain real members in All members; no subtree expansion is implicit. Navigating folder mode still applies its established file-only current-folder policy. An active narrowing label remains visible even when query/folder intersection is empty. No live selected-only behavior, persistence, ranger basename identity, or full filter-stack parity is claimed.

The snapshot holds at most the listing's 100,000 real IDs. Closing/replacing the listing resets it; selection edits, Save/Restore, sorting and history do not silently rewrite it. No additional archive reads, extraction, filesystem mutation or clipboard writes occur.

Two regressions exercise actual filters and folder trees, duplicate identities, hidden/cross-folder selection, snapshot stability across deselection, refresh, sorting, empty/invalid IDs, synthetic exclusion, regex failure and reset.

Native Narrow/Remove clicks, review workflow and layout remain pending because desktop CUA is unavailable. Earlier archive native gates remain open. No native capture success is claimed.

Validation:

- Initial full locked visual-qa suite: **2758 passed, 3 ignored, zero failed** (`test.log`).
- Final-source rerun: **2758 passed, 3 ignored, zero failed**, after the hidden-selection-count correction; retained separately (`test-final.log`).
- All-target visual-qa Clippy with warnings denied, locked default check and visual-qa build passed (`clippy.log`, `check.log`, `build.log`).
- Formatting and diff whitespace checks passed (`fmt.log`, `diff-check.log`).

Source/binary hashes and the pending native-validation record accompany this report.
