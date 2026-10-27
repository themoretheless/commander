# TAR and TAR content search — 2026-10-27

Scope: plain TAR browse/extract, TAR/tar.gz content search, bounded cancellable
sequential TAR scanning, archive labels and the archive pane's footer layout.
Inspired by the multi-format workflows in ouch and stream-based archived-file
access in mholt/archives. Independent implementation, no copied upstream code.

## Functional verification

- `cargo test tar -- --test-threads=1`: **124 passed**, no failures. Includes new
  plain/gzip TAR content fixtures, Unicode paths, timestamps and total declared
  sizes, binary/oversized text skips, stream limits for both listing and content
  reads, cancellation while skipping payloads, link targets not dereferenced,
  literal-name plain-TAR extraction and existing-file preservation. The filter
  also includes unrelated names containing `tar` (such as `target`/`start`); it
  must not be interpreted as 124 archive-specific cases.
- `cargo test search::tests -- --test-threads=1`: **31 passed, 1 ignored**. Includes
  a real recursive query finding one member in plain TAR and one in tar.gz, virtual
  locations/reveal paths, corrupt archive reporting and existing ZIP behavior.
- Final `cargo test archive -- --test-threads=1`: **50 passed**, no failures,
  including supported-format activation through workspace requests.
- Final focused shared member/text-budget test with visual-qa enabled: **1 passed**.
- Native harness unit checks: **3 passed**; scenario matrix is now nine cases.
- Final Clippy across all targets/features, warnings denied: passed.
- Formatting and whitespace checks: passed.

Filters overlap. Do not add their counts as a unique-test total. The earlier TAR
and archive runs preceded the footer-only layout repair; the final native capture
and Clippy cover that repair. The final focused budget case was added afterward.

## Native verification

`cargo run --features visual-qa -- --visual-qa archive_browser --output target/visual-qa-tar-content`

The new CI scenario creates a real two-member TAR fixture, opens the archive tool,
and waits for an actual complete TAR listing containing `docs/notes.txt` before
capture. The retained [frame](archive_browser/frame.png) was visually inspected:
two file rows, TAR label, sizes, unavailable packed sizes and both extraction
controls are visible. The manifest reports captured, all 14 geometry/pixel checks
passed and all recording-native-effect counters are zero. The real extraction
adapter is verified by functional tests; the capture does not click extraction.

An initial frame failed the right-pane geometry check: footer controls extended
30 logical points below the viewport. It and its failed manifest/log are retained
under `rejected-native`. The layout was fixed; the check was not relaxed.

## Bounds and limits

Text limit: 1 MiB/member, 32 MiB/run. Sequential decoded stream: 128 MiB/TAR scan,
including skipped payloads; reaching the stream/member cap reports partial results.
Cancellation is checked on reads capped at 64 KiB. This does not qualify hard
real-time cancellation of blocked filesystem I/O or a bounded extraction size.
TAR extraction continues to depend on the existing system `tar` adapter.
No xz/bzip2/zstd support, nested archive traversal, archive editing, universal
VoiceOver/release qualification or performance advantage is claimed.
