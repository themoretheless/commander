# SHA3-256 and SHA3-512 file checksums

Independent adaptation of [PeaZip checksum algorithms](https://peazip.github.io/pea-archiving-utility.html) (cohort entry 96), consulted 2026-11-03. No PeaZip source copied. RustCrypto sha3 0.12.0, keccak 0.2.2 and sponge-cursor 0.1.0 are added with their MIT license texts; no policy exception.

The guarded cancellable reader updates both SHA-3 algorithms in the existing 64 KiB file pass. Report, UI algorithm menus, Hex/Base64 copies and expected-value comparison, GNU list export/import and tagged detection share Algorithm::ALL. SHA-256 remains default. This is SHA-3, not Keccak. No throughput improvement claim: two additional digest computations add CPU work.

Supported tags are SHA3-256/SHA3_256 and SHA3-512/SHA3_512, with matching .sha3-256/.sha3_256/.sha3-512/.sha3_512 extensions and SHA3-256SUMS/SHA3_256SUMS/SHA3-512SUMS/SHA3_512SUMS filename hints. Untagged 64/128-digit lists are inherently ambiguous and require explicit selection or a filename hint. SHA-2 tags are distinct even at identical digest widths.

The Python hashlib/base64 oracle covers empty, abc, 65535/65536/65537 and 700001-byte patterned inputs. Rust tests compare exact oracle values, both expected-value encodings, mismatch/cancel refusal, list export and actual verification. Tag/hint tests reject SHA-2 confusion and ambiguous lengths; existing all-algorithm tests exercise escaped/relative/absolute export and guarded source handling. generate_oracle.py independently recalculates the stored vectors.

Preliminary tests exposed obsolete assumptions that SHA3-256 was unsupported and unhinted 128-digit lists uniquely meant SHA-512; those tests now reflect the expanded supported set. Failed logs retained. Native pointer/layout and native export qualification remain separate scope.

The dependency audit also found the existing async-executor 1.14.0 release had been yanked. A targeted lockfile update selects 1.14.1; no other existing versions were updated. The initial audit log is retained.

Final validation: all seven gates exited 0; tests 3359 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed. Ordinary check retains the pre-existing diff_find.rs unused-response warning. verify.py independently recalculated the Python oracle and checked terminal results and source hashes.
