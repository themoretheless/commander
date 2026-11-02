# Disk-usage JSON and CSV reports — 2026-10-31

Independently adapts [gdu's machine-readable analysis export](https://github.com/dundee/gdu#usage), cohort #84. This is Commander's own report schema; gdu import compatibility and a raw recursive file inventory are not claimed. No upstream implementation is copied.

## Behavior

**Copy scan JSON** is available in either Map or Tree after a valid snapshot returns. It exports schema 1, the captured root, exact root summary counters/logical bytes, retained top-level map items (including file rows), retained compressed directory endpoints, and cancellation/tree-cap/map-cap/unreadable-path metadata. It identifies the inaccessible-path list's own truncation separately. It ignores the active tree filter. Directory rows carry native absolute/relative paths, compressed labels/depths, raw byte/file counts and unreadable state. The root is separate from these nested totals; `directory_rows_are_nested_totals` explicitly warns that they are not summable independent buckets.

**Copy visible CSV** exports the shown Tree rows in source/view order, with ancestor context marked. Fields are `root,path,relative_path,depth,logical_bytes,files,unreadable,context,query,fuzzy,match_case,scan_cancelled,tree_truncated,inaccessible_count`. Sizes remain original subtree measurements, not filtered sums. A fresh pure projection check rejects stale, dirty or malformed view indices rather than exporting different rows. Empty visible views cannot be copied.

Both paths use the existing application clipboard owner, perform no new filesystem reads/scans and leave the snapshot/filter unchanged. Copying is disabled while rescanning and after a worker-level error; completed/stopped snapshots with explicit partial metadata remain exportable. UTF-8 names, quotes, commas and embedded line breaks are preserved; non-UTF-8, NUL, parent-traversing, outside-root or otherwise invalid report paths are refused. JSON uses integer byte values (including u64::MAX) and CSV uses decimal text; there is no float size conversion. Bounded serializer writers enforce a 1 MiB output limit during serialization and return errors rather than partial copied strings. Large reports require a narrower scan root or filtered CSV.

This exports the existing retained representation: up to 50000 compressed directory nodes and 512 map items, with partial flags. It does not reconstruct individual deep files omitted by that model, persist/reopen scans in the UI, write a user-selected file, or establish current filesystem state after the scan.

## Verification

Final source-frozen checks: **2922 passed, 3 ignored**, visual-qa build, all-target Clippy with warnings denied, default cargo check, format and diff checks, all exit zero in `checks.json`. The initial green pass is retained in `initial-pass`; the final rerun also includes disabling copies after worker-level errors.

Five new tests verify exact u64 integers and complete model scope, all partial/cancelled/capped/inaccessible flags, CSV quoting of comma/quote/newline paths, current row order and context markers, positive CSV partial-state metadata, unchanged subtree/root totals, stale/dirty/corrupt projection refusal, invalid/outside/non-UTF-8 paths and both serializers' actual output caps. Existing tree-filter pointer/path/indentation regressions remain passing with the new controls.

New native `disk_usage_export` CI scene (56 scenarios) uses the multi-branch real-file fixture. It copies JSON in Map, switches to Tree, filters target, copies visible CSV, then copies JSON again and requires byte-for-byte equality with the first unfiltered report. It adds one QA-only directory/file, presses Rescan, then copies the new JSON and CSV. Independent scene guards verify actual paths, source order, bytes, file/depth/context values, filter flags and total counts. Terminal phase 27 requires five recorded clipboard writes; native effects other than clipboard are zero. Root logical bytes change 246 -> 306, JSON directory/map rows 8 -> 9, while visible CSV rows change 4 -> 5 and keep two context ancestors. Unrelated gamma/unrelated remains in JSON but never in filtered CSV.

Fresh macOS eframe/Glow capture: **14/14 checks**. The final framebuffer was visually inspected: both copy buttons, active target filter, three direct matches/five shown rows, full 306-byte root and preserved hierarchy are visible. Launch/debug/app-core hashes matched. The short-lived app exited before observation reported application-not-found; its fresh unique manifest establishes completion. All five actual clipboard payloads are retained beside this report:

- [Initial scan JSON](scan-before.json) and [same scan while filtered](scan-filtered.json), required byte-identical.
- [Initial filtered CSV](visible-before.csv).
- [Rescanned JSON](scan-after.json) and [rescanned visible CSV](visible-after.csv).

These files were additionally parsed with Python's standard json/csv libraries, independently of the Rust serializers, checking counters, scope, context rows and omission/inclusion of the unrelated branch. `native-validation.json` records the result. Fixture paths refer to temporary data removed after capture.

Actual OS clipboard delivery, physical input, VoiceOver, cross-platform rendering, native stopped/partial/error/over-limit UI scenes and filesystem report-file saving are outside this evidence. Partial/error/limit behavior has model-level coverage and explicit UI guards. Full 99-repository parity remains open.
