# BLAKE3 checksum-list export — 2026-10-31

Continuation of the Double Commander checksum-file workflow (cohort #3), independently adapted to Commander's existing Rust BLAKE3 owner. Sources inspected: https://doublecmd.github.io/doc/en/help.html and https://github.com/BLAKE3-team/BLAKE3/blob/master/b3sum/src/main.rs . No upstream application implementation copied.

Previously choosing BLAKE3 silently kept the list action on SHA-256. The list button now follows the explicit algorithm, exporting b3sum's two-space untagged format rather than SHA's binary-mode star marker. Existing SHA export remains unchanged. Backslash/newline/carriage-return paths use the leading escape marker and escaped filename. Non-UTF-8, NUL, relative paths and BLAKE3 paths containing U+FFFD fail the entire export. Skipped/error/malformed/unprocessed rows are omitted; previously completed rows remain exportable after cancellation. No new payload reads or app dependencies.

Regression uses actual hashing for empty/abc/multi-chunk data and spaces, backslashes, newlines, carriage returns and Unicode names. With COMMANDER_B3SUM_VERIFY explicitly set, the exported file must pass official b3sum --check, then fail after payload mutation. Another regression covers ambiguous-path refusal and absence of SHA fallback.

Native Checksums extends the existing SHA-256/BLAKE3 comparison and SHA-384/512 copy sequence. It requires the old fourth SHA-512 clipboard payload before selecting BLAKE3, entering its expected digest and copying the list. Final capture requires phase 51, exactly five recorded clipboard writes, BLAKE3 match and the exact BLAKE3 list payload. These are instrumented native effects, not a claim of physical OS input/clipboard delivery or manifest import support.

Fresh CUA-launched native capture passed all 14 checks with exactly five recorded clipboard writes; phase 51 requires the exact BLAKE3 checkfile payload. Frame inspected: BLAKE3 selected, expected digest matches, Copy BLAKE3 list (1) visible and copy toast shown. Launch/current binary SHA-256 verified. CUA returned running-app-not-found after successful capture exit; unique fresh artifacts confirm completion.

Official crates.io b3sum 1.8.7 was installed only under /tmp for qualification. The explicitly enabled interoperability test passed all six files and required nonzero exit after mutation. Version/hash/provenance and logs are retained; application needs no installed CLI.

Final validation: locked visual-qa build passed; 2880 tests passed, 3 ignored, 0 failed. All-target Clippy with warnings denied, default-feature cargo check, rustfmt and git diff whitespace checks passed. Source hashes unchanged through qualification; exact commands/exit codes retained in checks.json.
