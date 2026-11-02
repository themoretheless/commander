# Disk-usage percentage bars — 2026-10-31

Independently adapts [dust's directory size percentages and bars](https://github.com/bootandy/dust#overview), cohort #82 in the researched 99 repositories. No upstream implementation is copied.

Tree mode now shows a percentage and a bar beside each directory's existing byte/file totals. **Share of Root** is the default; **Parent** uses the nearest represented ancestor in the full retained snapshot, falling back to the root for top-level compressed endpoints. This is explicitly the represented parent: physical intermediate directories can be omitted by chain compression. The switch survives Map/Tree and Rescan for the open tool. It performs no filesystem reads and changes neither the snapshot nor JSON/CSV report schemas.

Ancestor lookup is cached with the tree view and rebuilt on snapshot invalidation. Filtering does not change denominators, including when an ancestor is retained as context. Percentages use integer u128 multiplication/division of original u64 measurements, displayed as truncated tenths. A zero denominator shows an em dash. Only bar geometry clamps values above 100%; the actual percentage remains visible if a partial/inconsistent snapshot supplies a smaller denominator. Existing partial/cancelled/unreadable warnings remain, and bar tooltips identify exact logical bytes, the denominator path/root, and partial status. Nested totals must not be summed. CSV continues to export raw measurements and filter/partial metadata, rather than presentation percentages.

## Verification

Source-frozen validation: **2923 tests passed, 3 ignored**; visual-qa build, all-target Clippy with warnings denied, default cargo check, formatting and diff validation all exited zero. See `checks.json` and command logs. `source-sha256.json` records the four touched source files.

A new arithmetic regression covers zero totals, thirds, exact u64::MAX totals and over-100-percent overflow-safe values. The existing headless pointer regression now checks both denominators on a filtered compressed child: 33/200 = 16.5% of root and 33/132 = 25.0% of its represented parent. It still checks left indentation, original path navigation, unchanged hidden rows and unchanged measurements. The cached filter regression checks represented parent indices across compressed/nonancestor paths.

Fresh macOS eframe/Glow `disk_usage_export` now advances through phase 30. It retains all five actual clipboard report checks before/after filtering/rescanning, then checks the filtered target's root share and clicks Parent through settled native egui input. Terminal guards require Parent selected, target-one 10/30 = 33.3% (previously 10/306 = 3.2% of root), and compressed top-level delta 60/306 = 19.6%. Thus the ancestor denominator is the full snapshot value, not a sum of matched rows. The framebuffer was visually inspected: bars/percentages, hierarchy, sizes and controls are visible without overlap.

**14/14 native checks passed**, five recorded clipboard writes, all other native effect counters zero. `native-launch.json` identifies the unique run and matching debug/app-core hashes. The short-lived application finished before CUA's observation and returned application-not-found; the unique captured manifest and phase log establish successful execution. Actual clipboard delivery remains outside the recording backend's evidence.

The five exported payloads are retained. `native-validation.json` additionally validates them using Python stdlib json/csv, checks unchanged JSON while filtering, 306-byte root, five visible CSV rows and independent percentage calculations. No live host directory is modified by the scenario; its controlled fixture is temporary.

Physical input, VoiceOver, cross-platform appearance and native partial/zero-total screenshots are outside this evidence. Zero/large/inconsistent arithmetic has unit coverage; partial scans remain explicitly partial. Full 99-repository parity remains open.
