# Per-member ZIP compression and encryption metadata

Independent Rust adaptation of PeaZip cohort #96, specifically its per-item compression/encryption metadata display:
https://sourceforge.net/p/peazip/news/2021/09/peazip-820---free-open-source-file-archiver-utility/

ZIP listing reads the compression method and encrypted flag from the existing raw central-directory entry. No payload decoding, password prompt, or new dependency is involved. Unsupported method identifiers remain visible in the method name (e.g. `Unsupported(222)`). Real listed directory entries retain their recorded metadata; synthetic tree directories have no ZIP metadata. TAR and synthetic entries report unavailable rather than falsely reporting unencrypted or stored.

The Member details section adds Compression method and Encrypted. An encrypted entry explicitly says password extraction is unavailable. This transfers metadata visibility only; password extraction and encryption-method identification remain open.

CSV appends Compression method and Encrypted after the existing columns, preserving the previous column positions. Known unencrypted members export `false`; absent metadata exports empty fields. Existing escaping, formula neutralization, selection identity and size bounds still apply.

Regression coverage includes actual stored/deflated ZIP listing, central/local headers modified to encrypted unsupported-method entries with deliberately undecodable payloads, UI detail values/password requirement, and CSV true/false/unavailable distinction. Existing TAR, tree and export tests cover unchanged paths. `run_checks.py` records all seven repository gates. Native visual acceptance of the two additional detail rows remains pending; no screenshot claim is made.

Final gates: all seven passed; 3377 tests passed, 0 failed, 3 ignored. Strict visual-QA Clippy passed. Default check retains the pre-existing unused-response warning in diff_find.rs. The initial-checks directory preserves the earlier pass before the native CSV expectation was updated; final logs cover the final source hashes.
