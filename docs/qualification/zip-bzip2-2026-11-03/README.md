# BZip2 payloads inside ZIP and ZIPX

Independent adaptation of PeaZip cohort #96 support for alternative ZIP codecs:
https://peazip.github.io/zip-files-utility.html
https://peazip.github.io/zipx-file-format.html

The existing zip-rs dependency now enables its BZip2 feature, reusing Commander’s already-locked bzip2 0.6 dependency and its existing licence. Cargo.lock changes only the zip dependency edge, with no package/version additions. Listing, content search, ZIP CRC checks and extraction share this decoder; no external archiver process is invoked. Case-insensitive .zipx recognition routes the container through the same archive paths and coarse Archive classification (facets, saved type queries, grouping).

This adds BZip2 to existing Stored/Deflate ZIP decoding. It does not claim support for every ZIPX compression method. Zstandard, XZ, LZMA, PPMd and Deflate64 inside ZIP and password extraction remain open. In particular, ZIP decoder construction has different allocation controls from Commander’s bounded TAR adapters; those require their own integration review, rather than assuming decoded-byte limits also bound allocation.

Python standard-library zipfile independently produces deterministic fixtures: mixed Stored/Deflate/BZip2 entries including Unicode paths and an empty BZip2 payload; bad CRC; parent traversal; high compression ratio. make_fixtures.py and the manifest preserve input bytes, methods and hashes; verify_fixtures.py reads and checks valid members and expects Python’s bad-CRC failure. No fixture is extracted by the Python verifier.

Five new Rust regressions exercise exact listing/content/decoded-byte counts, extraction bytes and existing-file preservation; CRC failure with partial-output removal; unsafe paths and skipped ratio bombs; live and indexed archive content search for .zip and uppercase .ZIPX; integrity byte limits and cancellation. The first focused build needed an explicit sum::<u64> in a test assertion; its failed log is retained.

Native ArchiveMetadata is extended from 12 to 15 pointer phases. It opens metadata.ZIPX, retains Stored/Deflate/encrypted-unsupported display coverage and finally clicks an additional BZip2 member. Read-only observations record all eight rendered fields and clip visibility at each boundary. verify.py checks all four fixed states and the common native gates against the final binary. It does not claim a native extraction action; production extraction is qualified against the independently authored Python fixture in Rust tests.

A first integration query searched container files as ordinary binary candidates as well as their members, yielding two expected content-skipped counts. The final integration test covers both queries: broad content search requires exactly two skipped binary containers, while the member-path query requires zero skipped candidates. Both live and indexed modes must return exactly the two Unicode virtual member locations. The unfiltered-query log is retained.

Native visual review also exposed an existing footer defect: Same extension used a nested enabled UI in a wrapped row, so its label could wrap letter-by-letter. Applying a blanket Extend style was rejected by the next capture because it enlarged the pane outside the viewport (the failed manifest/frame are retained). The final fix makes this one menu a directly allocated enabled button with Popup::menu, matching the existing Same name/folder controls. The inspected final capture shows the three menus as whole buttons on the next row, all metadata fields visible, and the pane within the viewport. The successful pre-layout capture and pre-layout gates are historical; final gates cover the actual UI fix.

Final result: all seven gates passed; 3382 tests passed, 0 failed, 3 ignored. Strict visual-QA Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. Final native capture 1793661623192821000 passed 14/14 common gates with zero native effects; all four metadata states and final binary hash passed verify.py. Python fixture verification passed. The final BZip2 framebuffer was inspected for metadata and whole-button wrapping.
