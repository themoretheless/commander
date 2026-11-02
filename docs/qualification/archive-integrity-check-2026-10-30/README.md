# Read-only archive check — 2026-10-30

Independent Rust adaptation of [Ark Test Integrity](https://github.com/KDE/ark/blob/master/part/part.cpp), cohort #99. Commander now wires Test archive / Stop check into the archive pane, using its own streaming readers rather than an external testing executable. No upstream implementation copied.

ZIP checks consume each member to EOF through the library's CRC reader and reject overlapping data. Encrypted/unsupported members or decoding errors do not yield a completed report. TAR checks validate parsed headers and member lengths, consume member contents without extracting them and read all padding/concatenated compression frames to EOF. Gzip verification uses MultiGzDecoder separately from the existing listing reader; bzip2/XZ/Zstandard reuse their bounded adapters. Ignoring zero TAR records lets subsequent concatenated TAR frames be checked too. An empty byte stream is rejected; a zero-record empty TAR is accepted. This is a header/stream check under the TAR reader's EOF rules, not full POSIX conformance.

Only a complete successful run returns a Report. ZIP CRC verification detects the deliberately corrupted stored payload in the regression. Ordinary TAR has no member-payload checksum: a deliberate payload mutation with a valid header still completes, and the result explicitly names that limitation. No cryptographic authenticity, malicious-content detection, extraction-path safety or continual monitoring is claimed. A report belongs to the regular file observed when that check starts, not an earlier listing snapshot.

Checks allow 100,000 logical members and 128 MiB decoded bytes. TAR logical payload bytes have a separate equal cap so sparse logical expansion is bounded as well. A one-byte EOF probe distinguishes an exact limit from overflow. ZIP progress counts decoded member bytes; TAR progress counts the entire decoded stream including headers/padding. ZIP central-directory parsing occurs before the member-count check and is not a claimed universal metadata-memory bound. XZ decoder-memory and Zstandard window settings remain owned by the existing adapters.

Regular-file admission rejects lexical symlinks. Unix opens use no-follow/nonblocking flags; path and descriptor metadata are compared before/after reading with identity/size/mtime and Unix ctime checks. A path replacement during a valid old stream is rejected before publishing success. This is metadata-based version validation, not an atomic filesystem snapshot. Cancellation is cooperative across decoded reads, raw TAR reads and ZIP raw reads/seeks, without a hard timeout for a blocked filesystem call.

One check is owned by this archive pane; extraction and check actions exclude each other. Closing/retrying/replacing the owner drops its Run and sets only that run's cancellation flag; late responses have no receiver. Progress is polled while running; cancellation yields no completed result. Checking performs no member-path filesystem writes, staging, clipboard or opener calls.

Seven new regressions cover ZIP CRC corruption/binary reads, all five TAR codecs/source preservation, corrupted/truncated trailers and concatenated frames, bad headers/truncated payloads and the TAR checksum limitation, exact byte/count fences/cancellation, replacement/symlink rejection, and asynchronous completion/cancel ownership. Initial focused run expected the word CRC while the library correctly returned Invalid checksum; the precise checksum assertion was corrected, rejected log retained in `focused-initial.log`.

Native UI validation is pending because desktop CUA is unavailable. No fresh frame, native Test/Stop click, status layout or accessibility qualification is claimed. Earlier archive export/filter native gates remain open.

Verified core gates: full locked visual-qa suite **2743 passed, 0 failed, 3 ignored**; all-target visual-qa Clippy with warnings denied, locked default-feature check, locked visual-qa build and fmt/diff checks passed. Source/binary hashes accompany this report. Native UI validation remains pending as described above.

Broader transfers remain open; no commit or push.
