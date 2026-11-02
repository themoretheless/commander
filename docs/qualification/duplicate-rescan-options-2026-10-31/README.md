# Duplicate rescan preferences — 2026-10-31

Rescan now retains the selected keep policy and group sort. It clears all previous groups, index order and manual keep/skip decisions before starting the worker, then reapplies the selected policy and sorting to its newly hashed groups. Fresh opening retains the shortest-path / scan-order defaults. Both empty and nonempty result views use the same restart path. The tooltip explains that manual choices reset.

This is a follow-up correction to the transferred duplicate retention/sorting workflow, not a new upstream parity claim. The worker continues to use the active pane's listing candidates and captured metadata. New directory enumeration and refreshed modification timestamps are outside this change.

## Regression

`rescan_retains_preferences_rehashes_and_rebuilds_group_choices` uses real temporary files and the asynchronous production hashing worker. Five listed files initially form two groups. After selecting newest / potential savings and overriding both groups, one large file's payload changes without changing its size. Rescan must remove that old group, discard the small group's old skip and select its known newest member. The resulting proposal contains two files / 20 bytes. Changing the remaining small payloads to unique values then verifies fully empty results without preference loss. The test verifies pending scans expose no stale groups/order/choices. No Trash operation is run.

## Validation

`cargo test --locked --features visual-qa`: **2898 passed, 0 failed, 3 ignored**. Clippy (all targets, visual-qa, warnings denied), default cargo check, cargo fmt --check and git diff --check all returned zero.

Check commands and exit codes are recorded in `checks.json`; logs are adjacent. Native GUI interaction and screenshots were not rerun for this change. Earlier milestone screenshots do not qualify this new restart behavior.
