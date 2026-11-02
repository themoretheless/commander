# Undo/redo duplicate proposal edits

Commander extension inspired by [Kiorg undo/redo](https://github.com/houqp/kiorg#key-features), applied specifically to the duplicate review proposal. Undo plan edit and Redo plan edit restore keeper choices, Keep all groups and the selected automatic keeper policy. Manual radio choices, automatic Apply, Keep all and captured-folder preferences enter the same history. Report/path-list copying and group sorting do not create edits. This is not filesystem operation undo and does not restore trashed files.

History is in memory for the current scan only. A new scan state or publishing a scan clears it so old member indices cannot apply to new results. A branch after undo clears redo; no-op edits preserve redo. Undo/redo themselves move snapshots without recording a new edit. Up to 32 snapshots and 65,536 stored keeper choices are retained; older snapshots are evicted. Plans exceeding 65,536 groups cannot retain history. Current plan changes still work and no extra unbounded history allocation occurs. Snapshot length mismatches clear history rather than restoring an unrelated plan. Closing the tool discards history.

Two regressions verify exact keeper/Keep-all/policy round trips, branching, no-op handling, explicit reset, step/storage bounds and mismatched-plan refusal. Existing duplicate regressions cover per-member action/report ownership and scan publication. Native interaction and layout for these two new controls are not qualified in this milestone. No dependencies were added. Full Kiorg filesystem-operation parity and the wider cohort goal remain open; prior dependency audit failures are unchanged.

Final validation: all six gates passed, full suite 3285 passed / 3 ignored; source and executable hashes rechecked. New native Undo/Redo interaction/layout remains unqualified.

Later native Undo/Redo qualification: [actual pointer workflow and full report equality](../duplicate-plan-history-native-2026-11-02/README.md). The native gap above describes this earlier snapshot.
