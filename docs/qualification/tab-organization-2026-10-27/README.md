# Tab organization qualification — 2026-10-27

Scope: move tabs left/right and close duplicate folder tabs, via tab context menus
and the shared command palette. Independent implementation inspired by the
corresponding actions in Double Commander `src/fmain.pas`, inspected on this date.

Focused checks cover all 27 active/from/to combinations in a three-tab group,
slot binding and visible state preservation, pane isolation, active duplicate
retention, preservation of the first other path's filter, no-op cleanup,
command dispatch/availability, boundary rejection and modal guards.
Existing tab/session/command regression checks are included in the recorded logs.

Native context-menu navigation, VoiceOver operation and installed-release
qualification were not exercised in this increment. No performance claim is made.

Validation results:

- `cargo test tab -- --test-threads=1`: 57 passed, no failures.
- `cargo test command::tests -- --test-threads=1`: 46 passed, no failures.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- `cargo fmt --check` and `git diff --check`: passed.

The two test filters overlap; their counts are not a unique-test total.
