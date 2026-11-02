# Archive viewport-sized cursor navigation — 2026-10-30

Independent adaptation of [Yazi cursor movement by viewport height](https://yazi-rs.github.io/docs/configuration/keymap/#arrow), cohort #27 in the 99-project research. Yazi supports movement by a percentage of the screen height; Commander now supports whole member-viewport moves in its archive tool.

PageUp/PageDown, while the archive path filter has focus, move the member cursor by the number of complete row pitches in the bounded member viewport (minimum one). The calculation uses the same finite height and 30-pixel row body plus item spacing as the virtualized list. Existing one-row arrows stay available. Shift adds the traversed range through the same fixed-anchor selection owner, skipping synthetic navigation rows and retaining hidden selections. Page movement clamps at the list boundaries; an absent/hidden cursor starts at the first visible row, matching existing arrow behavior.

The shared input owner consumes only unmodified or Shift navigation keys while this filter is focused. Ctrl/Alt/Command keys are left for their other owners. The existing virtualized cursor-scroll request is set for page moves, including destinations outside currently rendered rows. No archive extraction, source read, sort or folder transition is started by movement.

Two new regressions cover saturated large steps, empty/hidden cursor recovery, arbitrary display ordering, synthetic row exclusion and hidden selection retention; real egui input events cover focus, key consumption, Shift and modifier rejection. Existing single-row cursor regressions remain. Initial rejected logs preserve a RawInput API mismatch and missing explicit ModifiersChanged test event; both test harness issues were corrected for egui 0.36.

Native PageUp/PageDown delivery, nested scroll reveal and Shift selection are pending. No CUA tool is exposed in this continuation; no native launch was attempted and older captures do not qualify this change. This report covers Rust/input behavior, not a native UI pass.

Validation: `cargo test --locked --features visual-qa` passed 2799 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, `cargo fmt --check` and `git diff --check` passed. Source and binary hashes are recorded alongside the logs. No commit or push performed.
