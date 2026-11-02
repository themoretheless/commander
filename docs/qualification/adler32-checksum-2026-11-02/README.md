# Adler-32 file and checksum-list compatibility

Independent adaptation of PeaZip file-checking capabilities (cohort entry 96): https://peazip.github.io/verify-checksum-hash.html, consulted 2026-11-02. No PeaZip code copied. Existing transitive `adler2` 2.0.1 is now a direct dependency, under its bundled MIT license; no version upgrade or policy exception.

The existing guarded cancellable file reader updates Adler-32 alongside the other digests in the same 64 KiB pass. Display/report, explicit digest comparison and copy, relative/absolute GNU-style list export, verification and algorithm selection share the new enum member. SHA-256 remains default. Adler-32 detects accidental errors; it is not a cryptographic digest.

Lists support ADLER32/ADLER-32 tags, ADLER32SUMS and .adler32/.adler32sum hints, plus untagged GNU text/binary record markers. CRC32 SFV remains filename-first and semicolon-comment-aware; Adler-32 never inherits those semantics. Tagged mismatches/misleading filename hints are refused. Existing path, symlink, changed-file, cancellation and byte/record limits remain in the shared lane.

Tests cover empty/abc/Wikipedia known values, 64 KiB boundaries and 700001 binary bytes against an independent weighted-sum mathematical definition; expected-value case handling, exact relative export, report text and cancelled-result refusal; list tags/hints/auto detection, mismatch and CRC32 separation. Existing Algorithm::ALL export/verification cases include the new algorithm. Native pointer/layout and OS pasteboard qualification remain open. No PeaZip algorithm-suite parity or speed claim.

The first test compilation exposed an exhaustive external-tool test selector needing its new algorithm arm; the failure is retained. Adler-32 joins its optional rhash lane, independently covered by the mathematical oracle regardless of tool availability.

Preliminary export expectation used text mode instead of the existing GNU binary marker and an existing detailed-report expectation omitted the new field. Both expectations were updated; failed logs are retained. Row allocation now derives its line count from Algorithm::ALL plus the content hash, rather than assuming eight lines.

Final validation: all seven gates exited 0, including dependency/license/advisory audit and strict Clippy. Tests: 3338 passed, 0 failed, 3 ignored. Ordinary check retains the pre-existing diff_find.rs unused-response warning. Complete logs and final source hashes are retained.
