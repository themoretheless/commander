# Recursive file allocation — 2026-11-02

Independent adaptation of [dust allocation versus apparent-size accounting](https://github.com/bootandy/dust/blob/master/README.md), cohort #82.

Disk usage now offers Logical size (default) and File allocation. Changing the metric re-scans the captured root, rebuilding the bounded top-512 map by the selected measurement. Tree ordering, shares, filters and exports consume the same measured snapshot.

File allocation counts Unix regular-file `st_blocks * 512`, once per device/inode identity. It excludes directory, symbolic-link and special-entry storage and does not measure reclaimable or exclusive shared extents. Traversal still excludes other devices and does not follow directory symlinks. Hard-link ownership follows existing scan order. Unknown observations and arithmetic overflow retain explicit incomplete status; shares with unknown or overflowing numerator/denominator are suppressed. Cancelled reports do not claim complete allocation totals.

JSON adds metric, measurement_scope, known_bytes and completeness metadata; logical_bytes is null in allocation mode. CSV retains its first 15 columns and appends six measurement columns; logical_bytes is blank in allocation mode. Consumers requiring an exact old CSV header must accept the added columns.

Regression coverage includes sparse files, real Unix block observations, unique hard-link accounting, symlink exclusion, zero/unknown/overflow, cancellation propagation, and selected-metric heap admission beyond 512 entries. Full checks are recorded in checks.json and individual logs; run_checks.py supplies independent checksum tools for the existing suite.

Native legacy disk_usage_export passed 14/14 checks with six intercepted clipboard writes; its screenshot was inspected. Two earlier phase-38 timeouts are preserved: the collapsed-CSV readiness helper still expected the old 15-column header. Updating it to the actual 21-column contract resolved the failure. No new native allocation-switch scenario is claimed. Further native qualification of the new switch is required before calling this milestone fully qualified.

Final validation: six checks passed; 3173 tests passed / 3 ignored. verify.py passed source/binary hash binding, native legacy 14/14, exact report totals and all three CSV measurement projections. Allocation-switch native qualification remains open.

Follow-up: native allocation-switch qualification is now completed in [the dedicated UI bundle](../disk-file-allocation-ui-2026-11-02/README.md). The pending statements above describe this earlier source snapshot.
