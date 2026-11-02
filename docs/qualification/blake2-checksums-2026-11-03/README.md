# BLAKE2s-256 and BLAKE2b-512 file checksums

Independent adaptation of [PeaZip BLAKE2S/BLAKE2B file checking](https://peazip.github.io/verify-checksum-hash.html), cohort entry 96, consulted 2026-11-03. No PeaZip code copied. Existing RustCrypto blake2 0.10.6 becomes a direct dependency; its MIT license is bundled, without version changes or policy exceptions.

The existing guarded, cancellable 64 KiB reader computes unkeyed full-width BLAKE2s-256/BLAKE2b-512 alongside the other file sums. Shared algorithm enumeration supplies digest display, comparison/copy selection and verification menus. Hex/Base64 expected-value comparison and copying, reports, absolute/relative GNU list export and list verification all use the new digests. SHA-256 remains default. No keyed, truncated, personalized or tree-hash BLAKE2 modes are exposed. These are distinct from BLAKE3.

Lists accept BLAKE2S/BLAKE2S-256 and BLAKE2B/BLAKE2B-512 tags, .blake2s/.blake2s-256/.blake2b/.blake2b-512 extensions and corresponding *SUMS filenames. Untagged 64/128-digit lists remain ambiguous and require an explicit algorithm or filename hint. SHA-2/SHA-3 and BLAKE3 tags remain distinct.

Python hashlib/base64 vectors cover empty, abc, 65535/65536/65537 and 700001-byte patterned payloads. Rust regressions check exact digests, display text, Hex/Base64 comparison and mismatch/cancel refusal, export and actual list verification. Tag/hint tests cover ambiguous widths and SHA-2 confusion; existing Algorithm::ALL cases cover path escaping, relocation and partial/changed-source refusal. generate_oracle.py recomputes the stored vectors independently.

Both algorithms add CPU work in the same disk pass; no speed improvement claim. Native pointer/layout/copy qualification remains separate scope.

Final validation: all seven gates exited 0; tests 3361 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the existing diff_find.rs unused-response warning. verify.py recalculated the independent Python oracle and checked terminal results and source hashes.
