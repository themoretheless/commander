# MD5 checksum interoperability — 2026-10-31

Independently adapts [Double Commander's checksum workflow](https://doublecmd.github.io/doc/en/help.html): MD5 calculation and explicit expected-digest comparison. Implementation uses [RustCrypto md-5 0.11.0](https://docs.rs/md-5/0.11.0/md5/), sharing the current Digest API. No upstream application code is copied.

## Behavior

MD5 is calculated in the same 64 KiB streaming loop as the existing hashes. It shares cancellation, regular-file/no-follow and before/after identity checks. Completed results expose MD5 in the report and algorithm selector. Explicit comparison requires 32 hexadecimal characters, accepts case differences, and distinguishes invalid, missing and mismatching values. Copy MD5 digest and Copy MD5 list use the existing application clipboard owner. SHA-256 remains the initial selection. MD5 supports existing legacy manifests; it is not an authenticity/signature feature.

MD5 manifests use GNU untagged binary records (`digest *filename`), with existing backslash/newline/carriage-return escaping and UTF-8 refusal. Relative paths reuse the common-directory binding and ./ prefix. Failed/skipped/malformed/unprocessed rows stay omitted. Calculation adds no extra file reads. No throughput improvement is claimed.

## Verification

All six checks in `checks.json` passed: visual-qa build, full visual-qa tests (**2900 passed, 3 ignored**), all-target Clippy with warnings denied, default cargo check, formatting and diff checks. Full tests ran with the recorded external RHash and b3sum paths enabled.

New MD5 vectors cover empty input, abc, 123456789 and one million a bytes (multiple read buffers). Each is independently compared with `/sbin/md5 -q`. Regression tests cover uppercase expected values, wrong algorithms/lengths/nonhex, mismatches, failed/cancelled/incomplete reports, exact manifest syntax and escaped names. Existing shared digest/report expectations were extended.

The real-file portable-list test now also exports MD5, moves the whole tree, and invokes RHash to accept its relative names (nested, Unicode, spaces, leading punctuation and trailing spaces); modifying a payload then makes verification fail. The external verifier is RHash 1.4.6, previously built from the official source; executable hash/version are retained in `native-validation.json`.

Native macOS eframe/Glow Checksums was freshly captured from the verified binary. The driver retains earlier SHA/BLAKE3/CRC32 gates, then uses pointer/key input to choose MD5, observe the old CRC32 expected text becoming invalid, enter an uppercase MD5 value, observe Match, copy the exact digest, copy a relative MD5 manifest, disable Relative paths and copy an absolute manifest. Terminal phase 103 requires the exact final payload and thirteen recording clipboard writes; intermediate gates verify the MD5 digest and relative payload as well. Manifest: captured, **14/14 checks**, all other native effect counters zero. Binary hashes matched the launch record and copied app core. `frame.png` was visually inspected: MD5 value, selection, uppercase expected text, Match and both copy buttons are visible.

The short-lived app exited after capture; the launch observer reported application-not-found afterward. The fresh unique capture and manifest prove the completed run. Actual OS clipboard delivery, physical user input, VoiceOver and broader cross-platform GUI qualification remain outside this evidence. This does not establish full 99-repository parity.
