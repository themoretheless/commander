# Duplicate keep-newest and bulk plan controls — 2026-10-31

Independent adaptation of [Double Commander's newest/oldest duplicate selection](https://doublecmd.github.io/doc/en/help.html), cohort #3. No upstream application implementation copied.

Keep newest joins existing shortest-path/oldest policies. Newest reverses only known-time ordering: a known mtime always beats unknown, equal newest times and all-unknown groups retain first input identity. Pre-epoch times remain valid. The shared keep_plan owner initializes results and replaces manual choices on policy selection or Apply policy. Keep all groups fills existing choices with None, preserving the selected policy; Apply policy can restore it without selecting a different rule. No rescan or I/O is added to these controls. Initial default remains shortest path and the existing explicit Trash operation is unchanged.

The removal iterator now skips an invalid keeper index rather than treating every file as a non-kept member. Empty groups and out-of-range keeper IDs produce no removal proposal. This also makes delete counts and reclaimed-byte totals agree with authoritative removal targets.

Pure regressions cover known/unknown/tied/pre-epoch times, stable IDs, complete reapply, Keep-all totals and malformed keeper refusal. A pointer-driven rendered-control regression verifies newest, Keep all groups, explicit reapply and oldest actions once per click. Existing duplicate-scan/full-digest/hardlink/identity behavior remains owned by its original subsystem.

Native duplicate_policy creates three equal-payload files with distinct mtimes and runs the real background scan. It requires shortest -> newest -> Keep all -> Apply newest -> oldest -> newest, checks mtime ordering/kept path/removal count at each checkpoint, then captures with all three payloads still intact and no operation/modal. All external effects must remain zero. The new scenario is in CI. Physical input, VoiceOver, native unknown/tied/multi-group/rescan variants and actual Trash execution remain open.

Final fresh CUA-launched phase-15 capture passed 14/14 checks with zero effects, including zero Trash calls. Frame inspected: newest selected, newest-copy.bin keeper at 1m versus 1h/2h peers, Apply policy and Keep all groups visible, two proposed files / 50 B. All bulk-policy checkpoints and intact payloads are mandatory before capture. Final/launch binary SHA-256 verified. CUA running-app-not-found followed successful short-lived capture.

Final validation: 2893 tests passed, 3 ignored, 0 failed. Locked visual-qa build, all-target Clippy with warnings denied, default-feature cargo check, rustfmt and git diff whitespace checks passed. Adding the third policy exposed Clippy enum_variant_names; internal variants were renamed Oldest/Newest/ShortestPath without persistence changes, and native capture was repeated. Initial logs/frame retained under initial-pass; exact commands and source/native hashes retained.
