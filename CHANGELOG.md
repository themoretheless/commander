# Changelog

All notable changes to Commander are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-09-15

### Added

- Structured logging via the `log` facade with a dual stderr + file subscriber
  (`config_dir()/logs/commander.log`) for durable background-failure trails.
- Headless `app/` smoke harness stub documenting a future `egui_kittest` path,
  plus a method-tabs render smoke test that runs without a native window.
- README notes for macOS Developer ID signing and notarization next steps.
- Phase 3 product surface: archive browse/extract, CLI launch paths, checksum
  verify, symlink/hardlink creation, run-command output capture, nav lock,
  dot-repeat, and Sync dry-run.
- Phase 4 research `J001-J010`: color-managed previews, version-store dedup
  quota, per-root trust labels, machine-pressure admission, encrypted support
  envelopes, assistive timeline, change provenance, workspace profiles,
  conflict rules, and decoder circuit breakers.

### Changed

- Package version bumped from `0.1.0` for the Phase 5 release-maturity cut.
- `cargo-deny` waiver for `RUSTSEC-2026-0192` refreshed after Phase 5 review;
  the Linux Wayland/`ttf-parser` path remains temporarily accepted through
  2026-10-28.

### Capabilities since 0.1.0

Highlights already on `main` that this release labels as 0.2.0 maturity:

- Durable copy/move/delete with Fast/Verified/Versioned profiles, operation
  journal, Recovery Center, and adaptive transfer telemetry.
- Async panel listing and generation-checked path probing off the UI thread.
- Shared `persistence::Persist` envelope for session, bookmarks, feature flags,
  version manifest, and the operation journal.
- Descriptor-relative filesystem effect port slice and placement/journal
  residual closure on the Phase 1 development line.
- Native visual QA matrix and fail-closed native release QA evidence path.
- Supply-chain gate via checked-in `deny.toml` and pinned `cargo-deny` in CI.

## [0.1.0] - 2026-07

### Added

- Initial dual-pane macOS file manager on egui: panels, bookmarks, search,
  transfers, compare/sync, and AppKit integrations (Quick Look, tags, share,
  APFS clone copies).
