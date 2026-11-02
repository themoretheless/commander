# Archive native export and bounded sheet — 2026-10-30

Continues qualification of the [Double Commander-inspired archive clipboard transfer](../archive-clipboard-export-2026-10-30/README.md), cohort #3. CUA is available again; this audit uses fresh native launches of the current binary rather than historical captures.

The corrected ArchiveExport index guard reached all five exact recording clipboard results, but the initial native run failed right_pane_rect: the right pane extended to y819.5 in a 760-high logical viewport. The rejected manifest/frame are retained in archive_export-initial-overflow. New control rows and details had outgrown the old estimated footer reserve.

The archive tool now places the whole sheet inside a vertical ScrollArea bounded by the real available pane height, with a zero minimum scrolled height. The member list remains independently virtualized with a finite viewport (35% of the original sheet height, clamped to 72–280 logical pixels), so an outer scrolling content UI cannot turn member rendering into an unbounded-height scan. Selection/extraction action rows wrap horizontally; details/errors/status can grow and remain reachable by scrolling. Current filters, sort, history, selections, IDs and worker/extraction owners are unchanged.

ArchiveExport now keeps its original five exact export actions and then sends a real scroll-wheel input over the sheet header, waiting for scroll settlement before final capture at phase 43. Final native status, source/binary binding and checked framebuffer are recorded after the fresh run. This uses recording clipboard isolation, not an OS pasteboard assertion, and does not extract files.

Fresh final ArchiveExport and ArchiveBrowser each passed 14/14 native checks with the same source-bound binary. Both frames were inspected. Export records exactly five clipboard writes with exact strings and no other native effects; Browser records zero effects. The export frame shows the sheet scrolled down with both extraction buttons inside the viewport. The ordinary browser frame shows the top controls and bounded member list inside the viewport.

The first bounded-sheet attempt exposed initial layout settlement: its first synthetic click preceded stable control placement and timed out at phase 3 with zero clipboard calls. That rejected run is retained in `archive_export-initial-settlement`. The final scenario waits six initial frames before the original click sequence. The earlier compile error for the required wheel phase and Clippy's collapsible-if rejection are retained in `test-initial.log` and `clippy-initial.log`.

Final full-suite/check results are in the adjacent logs. `native-validation.json`, per-scene `native-launch.json` and `source-sha256.json` bind the captured evidence to this source and binary. This closes only the archive export/browser layout gates: other archive interactions, actual extraction and the OS pasteboard still need independent qualification.

Validation: `cargo test --locked --features visual-qa` passed 2795 tests (3 ignored); Clippy with warnings denied, default `cargo check --locked`, visual-QA build, `cargo fmt --check` and `git diff --check` passed. Both own QA processes exited after capture. No commit or push performed.
