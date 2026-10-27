# Archive sorting qualification — 2026-10-27

Reference interaction: [Ark archive part](https://github.com/KDE/ark/blob/master/part/part.cpp)
uses ArchiveSortFilterModel and sortByColumn. Commander implementation is original
Rust/egui; no KDE source was copied.

## Verified behavior

Name/size/packed-size/compression-ratio/date order; primary-key reversal;
independent folders-first grouping; deterministic ties; unavailable TAR metadata
excluded from sorting; selection/cursor identities preserved; hidden-selection
membership remains correct after sorting and filtering.

`cargo test archive -- --test-threads=1`: **55 passed**, no failures. New cases
exercise metadata orders, reverse with folders first, exact near-u64::MAX ratio
comparison, field toggling/availability, focused-row identity and selected-member
visibility through filter changes. Existing archive extraction, filter and
activation regressions also passed. Final Clippy across all targets/features
with warnings denied, formatting and whitespace checks passed.

## Native frame

The existing `archive_browser` fixture now gives src/main.rs 83 bytes and
notes.txt 30 bytes, and chooses Size descending after listing publication. Capture
readiness requires that the rendered view's first index resolves to src/main.rs;
a default name-sorted frame cannot satisfy it.

The [retained native frame](archive_browser/frame.png) was visually inspected:
Size/Reverse controls, the 83-byte file above the 30-byte file, sortable headers,
modified dates and complete extraction controls. Manifest: captured; all 14
geometry/pixel checks pass; recording native effects are all zero. This verifies
actual sorted rendering, not every pointer/keyboard interaction or VoiceOver
operation. No performance advantage or full archive parity is claimed.

## Rejected evidence

An initial compile used the wrong name for the existing timestamp formatter; it
was corrected to modified_label. An initial native attempt chose the fixture's
sort before the listing worker published, which reset the member-view cache and
left a name-sorted view. The strict capture readiness gate rejected it. The fixture
now sets Size after publication and waits for a rendered sorted view; the gate
was not relaxed. Logs and failed manifest are retained under `rejected`.
