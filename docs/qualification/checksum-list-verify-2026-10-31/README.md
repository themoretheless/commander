# Verify checksum-list files — 2026-10-31

Independently adapts [Double Commander's Verify Checksum workflow](https://doublecmd.github.io/doc/en/help.html), cohort #3. No upstream application code is copied.

## Workflow

Highlight a checksum-list file and invoke **Verify checksum list** in the palette. The cursor-file command captures that path, preserving other selected files. Choose SHA-256, SHA-384, SHA-512, MD5, BLAKE3 or CRC32/SFV explicitly, then press Verify list. Algorithm choice is never inferred from digest length (SHA-256 and BLAKE3 have the same length). Changing it clears the old report.

The workload hash lane reads/parses the manifest and checks its records. Results distinguish MATCH, MISMATCH and ERROR, show expected/actual digests, counts and per-record errors, and copy a text verification report through the existing clipboard owner. Rerun reads current file contents again. Cancel requests worker cancellation; closing/replacing the dialog drops a task owner that cancels it. Algorithm controls are frozen while a task runs. A queued abandoned worker reports cancellation/stoppage, never a successful empty report.

GNU untagged SHA/MD5 records accept binary (` *`) and text (`  `) markers, optional leading escape marker and backslash/newline/carriage-return escapes. b3sum uses its two-space marker. SFV uses a filename plus final CRC32 token and semicolon comments; ordinary # comments, UTF-8 BOM and CRLF records are supported. Names retain spaces and Unicode; malformed records are rejected with a line number before target hashing. UTF-8 input is limited to 1 MiB / 10000 records. BSD tagged MD5, SHA1, signatures and other upstream algorithms/formats are not claimed.

Relative names resolve from the manifest folder. Absolute records are accepted only within that same folder, including its canonical spelling. Parent traversal, outside absolute paths and known symlinks in record components are refused. Missing/non-regular/unreadable/changing targets produce Error. Manifest reads require a regular file, use no-follow open on Unix, size bounds and before/after shallow identity checks. Target digests reuse the existing cancellable one-pass checksum worker and snapshot guards. Component checks are ordinary filesystem checks, not a descriptor-pinned sandbox against concurrent parent replacement.

## Verification

Final source-frozen checks: **2912 passed, 3 ignored**, visual-qa build, all-target Clippy with warnings denied, default cargo check, format and diff checks, all exit zero. Commands/logs are adjacent in `checks.json`.

New real-file regressions round-trip absolute and relative exports for all six algorithms, including nested/space/punctuation/control-character names where representable; check uppercase expectations, match/mismatch/missing statuses, wrong algorithm refusal and fresh rerun after payload changes; validate escapes/CRLF/SFV comments, invalid markers, bad escapes/NULs, size/record bounds and whole-manifest parse rejection. A system `/usr/bin/shasum -a 256` generated list is independently accepted and detects later corruption. Outside/parent/file-link/directory-link/manifest-link paths are refused without producing target hashes. Cancellation before work and after one completed record retains explicit incomplete counts. A workspace regression verifies cursor-file command dispatch and parent-row availability without disturbing a separate selection.

New native `checksum_list` CI scene (54 scenarios): the actual workspace command opens the dialog; pointer events run verification of four known records. The driver independently requires two matches, one mismatch and one missing-file error, copies the exact report, changes only the QA fixture's mismatching payload, reruns and requires three matches plus one missing-file error, then copies the second report. Terminal phase 12 checks two recording clipboard writes and known file payloads. Fresh macOS eframe/Glow capture: **14/14 checks**, clipboard 2, other native effects zero. `frame.png` was inspected: selected SHA-256, all four paths, final digests/error/counts and Copy button are visible. This is a SHA-256 GUI scenario; other formats have worker-level coverage.

Fresh existing `checksums` regression from the same binary: **14/14 checks**, terminal phase 103, thirteen exact recorded clipboard writes for calculation/comparison/export; other effects zero. `calculation-frame.png` was inspected. Both launch hashes match the debug binary and copied app core.

Two initial native attempts timed out and are retained under `initial-pass` / `second-pass`, alongside their green build/test logs. The driver was corrected to wait for actual cursor binding before command dispatch and stable widget geometry before pointer presses; result/clipboard requirements were retained. The final unique run logs every phase 0–11 and completes capture. Its launch observer reported application-not-found after the short-lived app exited; the fresh manifest proves completion.

Actual OS clipboard delivery, physical user input, VoiceOver, native cancellation/queued-admission failure, other-algorithm GUI scenes and broader cross-platform qualification remain open. No deletion or external opener was invoked. Full 99-repository parity is not claimed.
