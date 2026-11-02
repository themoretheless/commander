# Closed-tab history in the session

Commander continuation of the [Nimble Commander recently closed locations workflow](https://github.com/mikekazakov/nimble-commander/blob/ee4abffb165019106c7420db5ea8b51623e4baa4/Docs/Help.md). Persistence is a Commander extension; no claim that this exactly reproduces upstream persistence internals, and no upstream implementation copied.

Each pane now saves the newest eight closed tabs in the existing atomic session envelope. Restoring retains chronological order, original insertion positions, titles, full-path labels, view settings and per-path Miller widths. Reopen latest/index consumes the restored record through existing tab owners and capacity/modal guards. Startup creates suspended panels without listing or watching closed directories; unavailable paths can be reopened into the normal unavailable-directory state.

Missing/malformed history defaults empty; individual invalid/relative rows are skipped. Parsing scans at most the newest 256 rows to retain eight valid records, and clamps positions to the open-tab bound. Non-UTF-8 closed roots follow existing open-tab persistence policy: omit those optional records without mutating live history or preventing the rest of the session from being saved. Width keys retain their independent native encoding.

Evidence: real atomic session file save/load into fresh workspace objects, independent left/right histories, reopening an older indexed record and the newest record, insertion position, title/mode/full-path/width restoration, malformed/legacy/bounded rows, lazy unavailable roots and optional non-UTF-8-root omission. Six gate logs and source hashes are included.

Limits: live cursor/selection/scroll/history are retained when reopening within the process, but the saved history restores the same presentation snapshot as saved open tabs, not a full runtime panel snapshot. No new GUI process-relaunch capture was performed for this serialization milestone. The broader cohort scope remains open.

Final qualification: **3215 passed / 3 ignored**, all six gates exited zero, and `verify.py` passed final source/validator identities and the three new persistence regressions. The initial constant-visibility clippy failure is preserved under `failed-clippy`. Normal-feature check retains the unrelated unused `response` warning in `src/diff_find.rs:136`; strict visual-QA clippy passed.
