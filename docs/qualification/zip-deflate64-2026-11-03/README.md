# Deflate64 ZIP/ZIPX decoding

Independent integration of PeaZip cohort #96 alternative ZIP codec support:
https://peazip.github.io/zip-files-utility.html
https://peazip.github.io/zip-file-format.html
ZIP container reference: https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT

Enables the zip-rs Deflate64 feature. The lock adds deflate64 0.1.12, with no dependencies of its own; its complete MIT licence and .NET Foundation/anatawa12 attribution are preserved in data/licenses and THIRD_PARTY_LICENSES.md. Decoder history/state uses fixed-size buffers rather than an attacker-selected dictionary allocation. Existing listing, bounded archive content search, CRC checking and extraction use the same in-process method-9 decoder. Stored/Deflate/BZip2 decoding and case-insensitive ZIPX container routing remain available. This adds reading/extraction, not Deflate64 archive creation. Other ZIPX codecs and passwords remain open.

The fixtures deliberately exercise real Deflate64 extensions: length symbol 285 with sixteen extra bits for a 300-byte copy; distance symbol 31 with a 65536-byte back-reference; maximum 65538-byte match; and empty content. They are produced by an independently written minimal fixed-Huffman encoder, not by zip-rs or the Deflate64 dependency. `/usr/bin/unzip -p`, built with USE_DEFLATE64, independently verifies exact output bytes and hashes without filesystem extraction. Python zipfile inspects only ZIP headers (it cannot decode method 9). Bad-CRC and truncated streams must be rejected by the independent oracle. Generator, verifier, payload expectations, fixture manifest and oracle version are included. These are concrete extension cases, not ordinary Deflate bytes with a changed method number.

Five new Rust regressions exercise listing metadata, exact content and output bytes, total decoded-byte accounting, empty content, safe repeated extraction without overwriting, bad CRC/truncation with partial-file removal, maximum match decoding plus search-ratio/cancellation guards, live/indexed Unicode-path archive content search for ZIP and uppercase ZIPX, and integrity decoded-byte limits/cancellation. Existing archive safety tests remain in the full suite.

Native ArchiveMetadata extends from 15 to 18 pointer phases, preserving all four prior states and finally clicking a new Deflate64 member. Fixture setup raw-copies the independently encoded first entry rather than pretending zip-rs has a method-9 encoder. Native observations require all eight actual rendered metadata fields visible; verify.py checks five fixed states and expected 301-byte Deflate64 size. Native capture qualifies method visibility and the opening ZIPX container, not an extraction action. Rust tests qualify extraction against the independent oracle’s byte expectations.

Final verification: all seven gates passed; 3387 tests passed, 0 failed, 3 ignored. Strict visual-QA Clippy and dependency/license audit passed. Ordinary check retains the pre-existing diff_find.rs unused-response warning. Native receipt 1793662069533671000 passed 14/14 common checks with zero native effects, five exact rendered metadata observations and final-build hash equality. Its final Deflate64 framebuffer was inspected. The independent Info-ZIP fixture verifier passed.

Follow-up: [bounded LZMA reading](../zip-lzma-2026-11-03/README.md) adds ZIP method 14; earlier remaining-codec notes above describe the original Deflate64 milestone.
