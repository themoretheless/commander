# Recently closed tabs — qualification, 2026-10-27

Reference: [Ranger's core tab_restore](https://github.com/ranger/ranger/blob/master/ranger/core/actions.py).
Independent Rust/egui implementation. No copied upstream Python code.

## Verified behavior

- `cargo test tabs -- --test-threads=1`: **17 passed**, no failures. New cases
  restore directory, selection, history, cursor/filter/view state, invalidate old
  listing rows and discover a file added while the tab was closed; retain only
  eight recent panels; select a specific history entry; reject full-group and
  invalid-index attempts without consuming history; reconstruct interleaved
  deduplicated groups and their distinct filters; move history on pane swap and
  obey pending-modal guards. Existing tab/session regressions also passed.
- `cargo test command::tests -- --test-threads=1`: **47 passed**, no failures,
  including Cmd+T vs Cmd+Shift+T precedence, command availability and catalog
  uniqueness/shortcut contracts.
- Final Clippy across all targets/features with warnings denied: passed.
- Formatting and whitespace checks: passed.

History is scoped to a pane, capped at eight closed panels and kept only in RAM.
Session snapshots still contain open tabs only. Files themselves are never
removed by tab closing/reopening. Inactive/closed panel workloads are suspended;
reopening uses the existing activation refresh instead of publishing stale rows.

## Native frame

The existing folder_tabs native fixture now performs a close/reopen cycle,
then leaves one additional recently closed tab. Readiness requires three open
left tabs, third active, and one closed-history entry.

The retained [frame](folder_tabs/frame.png) was visually inspected: all three tabs,
selected third tab and Closed (1) menu are visible. Native manifest: captured,
14 checks passed, recording-native-effect counters all zero. The frame covers
rendering after programmatic close/reopen and history-menu presence; it does not
claim every menu-pointer/VoiceOver interaction or release qualification.

An initial Clippy run rejected two test-only style issues; both were corrected
without lint suppression. Its rejected log is retained. A failed text-replacement
script applied no edits; the intended accessibility-label and fixture changes
were then applied and verified against current files before the accepted capture.
