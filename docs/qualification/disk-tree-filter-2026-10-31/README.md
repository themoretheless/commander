# Disk-usage tree filtering — 2026-10-31

Independently adapts [broot's directory-tree filtering and navigation](https://github.com/Canop/broot#find-a-directory-then-cd-to-it), cohort #31. No upstream implementation is copied.

## Behavior

In Disk usage (`Cmd+Shift+M`), choose Tree and type a root-relative directory-path query. Default matching is literal substring, with optional existing Commander Fuzzy subsequence matching and Match case. Clear empties the query and returns focus to it. Matching is against the loaded compressed directory-node endpoints' relative paths, including their compressed chain components, rather than the absolute root prefix. The original root is not itself a tree row.

Ancestors represented by other rows remain visible as `(context)` without being counted as direct matches. Counts distinguish matches, shown rows including context, and loaded compressed nodes. Source preorder/size ordering, depths, byte/file totals, unreadable state and navigation paths remain bound to original nodes. Totals always describe the complete scanned subtrees, including hidden-by-query descendants; they are not recomputed over visible rows. Context rows remain navigable through the existing behavior; unreadable rows remain guarded.

Filtering is pure Rust over the last published snapshot and adds no filesystem reads or scan. Query/options and empty results are cached. New completed or stopped snapshots invalidate the cache; changing the query resets tree scrolling. Filters survive Map/Tree switches and Rescan, while Map continues to show its normal unfiltered map. Freshly opening the tool starts with a blank filter. Matching is bounded to the existing loaded snapshot (up to 50000 directory nodes) and 256 query characters. It is synchronous local matching on option/snapshot changes, not broot's ranking/query language, file search, ignore-file rules or asynchronous per-keystroke traversal. No performance superiority is claimed.

## Verification

Final source-frozen checks: **2917 passed, 3 ignored**, visual-qa build, all-target Clippy with warnings denied, default cargo check, formatting and diff checks, all exit zero. Exact commands/logs are adjacent in `checks.json`.

New model regressions cover compressed-chain ancestor retention, absence of similarly prefixed non-ancestors, stable source indices/order/depth/totals, Unicode literal case behavior, fuzzy subsequences, root-relative matching, cached empty results, same-length replacement after invalidation, whitespace clearing and overlong-query refusal. A headless egui pointer regression hides a row, clicks the resulting second visible node and requires its original source-index-2 path, not source-index-1. It also asserts actual child-label indentation and retained subtree totals.

New `disk_tree_filter` CI scene (55 scenarios) creates a physical multi-branch tree. Pointer/key events choose Tree, enter uppercase TGT with no literal results, enable Fuzzy (two matches/four shown), enable Match case (empty), disable it (matches return), Clear (all rows), enter target and switch back to literal. It then adds only a QA fixture directory/file, presses Rescan and requires three matches/five shown from the newly published snapshot. Root logical bytes must change from 246 to 306, while the filter does not rewrite totals. Map/Tree round trip must retain the query/options and results. Terminal phase 37 validates exact expected paths and unchanged fixture payloads.

Fresh native macOS eframe/Glow capture: **14/14 checks**, all native effect counters zero. `frame.png` was visually inspected: literal target query, three matches, five shown rows with labelled ancestors, nine loaded compressed nodes and complete 306-byte root are visible. The final labels are left-aligned and child indentation is visible. Launch/debug/app-core hashes matched. The short-lived app completed its capture before the AX observation, which returned AXError.cannotComplete; the fresh unique manifest establishes completion.

The earlier functional captures/checks are retained in `first-pass` and `second-pass`. Visual inspection exposed centered node labels; setting Label alignment alone did not override add_sized's centered child layout. The final renderer uses an explicit left-to-right child layout, with a new geometry assertion and a full source-frozen rerun/capture.

Native physical keyboard input, VoiceOver, cross-platform rendering, large-tree responsiveness and native folder-navigation clicks are not established by this run. Folder-click binding is covered by the rendered headless pointer test. Existing scan/truncation/cancellation boundaries remain in force. Full 99-repository parity remains open.
