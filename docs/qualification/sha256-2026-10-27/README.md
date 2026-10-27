# SHA-256 calculation and comparison — 2026-10-27

Commander independently adds SHA-256 calculation, expected-digest comparison for
one completed file, and clipboard export of checksum lists. The reference is
[Double Commander checksum calculation/verification](https://github.com/doublecmd/doublecmd/blob/master/src/fchecksumcalc.pas).
No upstream implementation was copied. SHA-256 uses the MIT/Apache-2.0
[RustCrypto sha2 0.11.1](https://docs.rs/sha2/0.11.1/sha2/) crate.

## Behavior

The existing **Verify checksum** palette command opens **Checksums**. Selected
regular files receive SHA-256, BLAKE3 and the existing short content hash from one
fresh read, without trusting or populating the existing verified-hash cache.
The short hash retains its previous big-endian BLAKE3 prefix format.

Paste exactly 64 hexadecimal characters into **Expected SHA-256** to compare a
single completed file. Surrounding whitespace and uppercase are accepted. The UI
distinguishes a match, mismatch and invalid input. Calculating a digest alone is
labelled “Calculated”, without claiming verification against a publisher.
Expected-digest comparison does not establish who published that value.

**Copy report** includes escaped full paths. **Copy SHA-256 list (N)** exports only
completed successful rows, with absolute UTF-8 paths and the GNU untagged binary
format. Failed, skipped and unprocessed entries are omitted, as stated by the
button tooltip and report counts. A non-UTF-8 path rejects the whole export
before any clipboard write; paths are never silently converted lossily.

The [GNU format specification](https://www.gnu.org/software/coreutils/manual/html_node/cksum-output-modes.html)
requires a leading backslash and filename escapes for backslash, newline and
carriage return. Tests verify exact carriage-return serialization. macOS
`/usr/bin/shasum -a 256 --strict -c` independently verified exported manifests for
spaces, backslashes, linefeeds and Unicode. Its Perl implementation does not
unescape GNU `\r`, so manifests containing a carriage-return filename cannot be
checked with that tool. Darwin `/sbin/sha256sum -c` also skipped this record as
improperly formatted while returning exit status 0. Neither result is treated as
successful verification of that filename; GNU coreutils was not installed or
qualified on this machine.

## Read and cancellation boundaries

Directories, symlinks and special files are skipped. Unix opens use O_NOFOLLOW
and nonblocking flags; regular-file metadata and path identity are checked
before and after reading. File replacement, truncation and growth reject the
current result. Reads use a 64 KiB buffer and cancellation checks between chunks.
A changing append tail cannot cause an unbounded read. Cancellation preserves
completed rows and discards the interrupted file's partial digests. The window
waits for the worker after Cancel; closing it requests cancellation and drops
the view. Cancellation cannot preempt a blocked OS read. Metadata checks are not
an atomic filesystem snapshot and cannot exclude undetectable concurrent writes.
No input files are modified. No performance improvement is claimed.

## Verification

The full default suite ran once: **2262 passed, 1 failed, 3 ignored**. The only
failure attempted to create an invalid UTF-8 filename on macOS, which the local
filesystem rejected before reaching the exporter. That fixture was corrected to
exercise export of a synthetic completed row. The prior failed logs are retained
under `rejected/`, including a caught short-hash endianness regression and the
system-shasum carriage-return limitation. This report does not claim a clean
full-suite rerun after those focused corrections.

Final focused checks: **15/15 checksum tests**, **1/1 scenario-matrix test**,
and Clippy with all targets/visual-qa and `-D warnings` passed. Logs accompany
this report. Coverage
includes standard SHA-256 vectors (empty, abc, million a), expected-value states,
BLAKE3/content-hash compatibility, cancellation inside a large file, path and
symlink substitution, special files, changing file length, lossless export and
system-shasum interoperability. Dependency policy passed advisories, bans,
licenses and sources; existing multiple-version warnings remain.

A real macOS eframe/Glow frame exercised the worker and pointer/text input into
the expected-digest field, reached **SHA-256 matches**, and passed **14/14** frame,
glyph, pane and input-isolation checks. Clipboard, opener and trash ports were
not invoked. `frame.png` was visually inspected. The new `checksums` scenario is
also included in the 14-case CI matrix; no remote CI run is claimed.
