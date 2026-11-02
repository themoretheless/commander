# Swap synchronization comparison sides

Independent adaptation of [Krusader synchronizer Swap sides](https://docs.kde.org/trunk_kf6/en/krusader/krusader/synchronizer.html). Commander's **Swap comparison sides** exchanges the comparison roots and their hidden-file flags. Mirror left-to-right becomes mirror right-to-left and vice versa, preserving the physical source/destination. Two-way, content comparison and dry run remain unchanged. Workspace panel paths and saved profiles are not changed.

The action is disabled during comparison and shares the profile loader's overlapping-scan refusal. It applies after the UI body has finished reading the old plan, returns before Apply handling, clears old rows/stamp/review/large-plan acknowledgement, then launches a fresh asynchronous comparison. Live guards, marker, durability and retention remain unchanged. Swapping does not copy files.

A new regression covers all three policy involutions, swapped hidden flags and preserved content/dry-run settings; a real asynchronous mirror plan proves the resulting ToLeft direction still reads from the original physical source, uses swapped stamped roots, clears the old authorization and does not copy or change source bytes. Existing loader regressions cover invalid roots and overlapping scans. Native pointer/keyboard interaction and layout for this button remain unqualified; no full Krusader parity is claimed.

No dependency changes. The broader 99-repository transfer objective and previously recorded dependency audit failures remain open. Gate logs and exact source/binary identities are recorded alongside this document.

The initial gate run is retained in initial-failed/: strict Clippy caught an overwritten swap request despite passing model tests. The editor result now replaces the pending request only when it supplies a profile. Final results refer to the rerun after that correction.

Final validation: all six gates passed; full suite 3279 passed / 3 ignored. Source and executable identities match the recorded hashes. Native button interaction/layout remains open.
