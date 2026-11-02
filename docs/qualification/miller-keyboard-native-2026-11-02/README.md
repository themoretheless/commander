# Native Miller keyboard qualification

Fresh CUA-launched owned application captures of MillerBrowser and FolderTabs. The Miller scene extends the prior 58-event pointer/resize/F3 route to 65 phases using egui key events through the real app input handler. No direct model navigation is used for the added transitions.

Four independent snapshots prove Right on `a.txt` leaves path/cursor/selection/listing revision unchanged; Left reaches the parent with `column-A` focused; Right returns to `column-A` with `a.txt` cursor/selection restored. Both resized widths remain 280/300. The inactive right pane stays on its fixture path. Previous nine route/F3 snapshots and seven measured resize states are checked again.

Both captures completed and their frames were visually inspected: Columns/current row and geometry remain visible; the neighboring folder-tab layout remains intact. The owned launcher uses a fresh Mach-O inode, without security setting changes. FolderTabs CUA attachment returned AXError.cannotComplete after launch; authoritative manifest/process inspection confirmed successful capture and terminal exit, so it was not relaunched.

`verify.py` checks all six build/test/static gates, source/binary/core/validator hashes, capture manifests and their 14 checks, zero external-effect counters, PNG dimensions and exact route/selection/geometry artifacts.

Limits: injected egui events in a native window qualify the app route; physical OS key delivery is not claimed. Independent ancestor keyboard focus/multi-selection, persisted closed-tab history and remaining cohort parity stay open. Session-width restart capture is still separate from this keyboard capture.

Final result: **3212 passed / 3 ignored**, all six gates exited zero, and both native workflows passed **14/14** with every external-effect counter zero. `verify.py` passed against final source/binary/core/validator hashes. The normal-feature check reports an unrelated unused `response` warning in `src/diff_find.rs:136`; strict visual-QA clippy is clean. The initial clippy failure and launch records are retained under `failed-clippy`; its earlier captures remain preserved in `native-captures`.

Final captures: `native-captures/1793595930460040000/miller_browser` and `native-captures/1793595959874913000/folder_tabs`.
