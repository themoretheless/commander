# Watcher link scope — 2026-10-31

Reliability correction discovered while qualifying the transferred recursive duplicate finder. It is consistent with [fd's explicit --follow option](https://github.com/sharkdp/fd#command-line-options), cohort #77, but does not copy or claim to port an fd watcher implementation.

## Problem and change

The earlier native `duplicate_recursive` fixture contains a child symlink back to its root. Its retained log has eight walkdir/loop/restart lines (`prior-native-evidence.json`, extracted log adjacent). notify Config defaults to following symlinks; even the shallow polling scanner can encounter a root cycle. Generic loop errors then trigger Commander's backend-restart path.

`install_backend` now constructs one explicit notify Config with the existing interval and `with_follow_symlinks(false)`, passed to both PollWatcher and RecommendedWatcher. The native convenience constructor was replaced with the configurable constructor. The notify version in this checkout documents the flag for inotify, kqueue and polling; FSEvents behavior is owned by its platform backend, so no universal native no-follow guarantee is claimed. Existing native/polling selection, depths, async install/teardown, admission limits, epochs, reconciliation and retry policies are unchanged. This changes watcher traversal, not navigation, file listing, search or file-operation link handling. Changes solely to an external linked target intentionally stop being recursive watch events; link nodes remain observed.

## Regression and validation

`polling_watchers_ignore_link_cycles_and_external_targets_but_detect_real_changes` exercises the production backend factory on real temporary folders with a directory loop, an external-directory alias and a file alias. It failed before the configuration fix at the initial-scan assertion (`regression-before.log`). The intermediate focused pass is retained separately; the final expanded test is in the full suite.

For recursive and directory-only depths, it checks no initial error, modifies an external target and an actual in-scope file, creates a new link node and manually polls the real PollWatcher. The recursive case observes a deep physical file; shallow mode observes a direct file. Both must observe the new link itself without reporting linked targets or restart/gap messages. An unwatch operation takes notify's scan locks before draining remaining messages and disposing fixtures, so the test checks a completed scan instead of assuming an arbitrary sleep proves absence. Explicit future mtimes avoid timestamp-resolution races. No shared global health-counter assumptions or fake watcher events are used.

Final checks: **2913 passed, 3 ignored**, visual-qa build, all-target Clippy with warnings denied, default cargo check, formatting and diff checks; all exit zero in `checks.json`.

Fresh macOS eframe/Glow `duplicate_recursive` capture from the verified binary: **14/14 checks**, terminal phase 21, two exact recording CSV clipboard writes and other native effects zero. The fixture retains the symbolic loop, nested/hidden duplicate groups, scope toggles, newest keeper and sorting. The new log has **zero** walkdir/loop/restart lines, compared with the eight lines in the earlier capture; this is bounded runtime evidence, not a long-duration stability or CPU benchmark. `frame.png` was visually inspected; the loop remains visible and results/actions remain intact. Launch/debug/app-core hashes matched. The app completed and exited before launch observation, which reported application-not-found; its unique captured manifest establishes completion.

Linux inotify, kqueue, physical OS input, OS clipboard delivery, VoiceOver, hostile concurrent symlink replacement and long-duration watcher qualification are not established by this run. This is a scoped correction, not full 99-repository parity.
