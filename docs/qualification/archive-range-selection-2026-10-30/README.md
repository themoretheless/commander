# Archive additive range selection — 2026-10-30

Independent adaptation of [Double Commander Shift selection](https://doublecmd.github.io/doc/en/help.html#sel_files), cohort #3. The upstream documents Shift-click and Shift-arrow consecutive selection. Commander applies that interaction to archive rows.

Shift-click and Shift-Up/Down in the focused archive filter add the displayed range from an anchor. Normal real-row clicks and unmodified arrows establish a new anchor; repeated Shift gestures retain it. Selection is additive, including when reversing direction: this does not shrink an already selected range. Existing plain clicks still toggle individual members. Synthetic navigation rows are skipped, including when used as range endpoints; Shift-clicking them selects the intervening real rows without navigating. Shift-double-click also avoids navigation. Real archive directory entries remain distinct extraction entries, following existing Select all semantics.

Range order follows the current visible sorted rows, so hidden/filter-excluded members are never newly selected and existing hidden selections remain. Stale anchors fall back to the endpoint; hidden anchors are cleared when rows are rebuilt. Direct navigation and history traversal reset the anchor. History does not persist range anchors. Existing cursor reveal handles keyboard range movement.

Two regressions cover reordered forward/reverse ranges, synthetic endpoints, hidden selections, invalid endpoints, stale anchors and actual query/navigation/history resets.

Native Shift gesture, focus and layout verification remains pending: desktop CUA is unavailable. Earlier archive native gates remain open. No native capture success is claimed.

Validation logs:

- `test.log`: initial full suite, 2751 passed / 3 ignored.
- `test-final.log`: final-source full suite, 2751 passed / 3 ignored; kept separately after the Shift-double-click guard and hint adjustment.
- `clippy.log`: all-target visual-qa Clippy with warnings denied.
- `check.log` and `build.log`: locked default check and visual-qa build.
- `fmt.log` and `diff-check.log`: formatting and whitespace checks.

All listed commands passed. See accompanying source/binary hashes and pending native-validation record.
