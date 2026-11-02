# UTF-16 LE/BE hex character column — 2026-10-31

Independent adaptation of Far Manager UTF-16 viewer character rendering and absolute-even code-unit alignment:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer dump/hex mode).

Characters now offers UTF-16 LE and BE with explicit byte order. Code units align to even absolute file offsets, not row/page starts. Valid surrogate pairs form one scalar. Commander retains one character-column position per source byte: leading scalar followed by › for remaining bytes. This differs from Far's one-position-per-code-unit representation and preserves Commander's existing byte-position column convention. Malformed/truncated units and odd EOF bytes use dots; BOM/control/combining/formatting and noncharacters are sanitized. Byte order is not inferred from BOM.

The existing bounded three-byte prefix/suffix context supplies cross-row/page and mid-unit data from the accepted snapshot. No new reads on encoding switch; raw byte exports, page counts/offsets, search encoding, history and bookmarks remain independent. Copy page uses the same character representation.

Two regressions cover both byte orders, BMP Cyrillic/Latin, surrogate-pair emoji crossing offset 4096, odd/mid-pair offsets, widths 1/3/16/64, EOF and malformed units, plus actual selector/Copy page pointer actions and unchanged raw data/search mode/generation/no queued work.

Dedicated native HexUtf16Encoding opens AПривет🌍Z encoded LE, selects Characters / UTF-16 LE, requires first row A›П›р›и›в›е›т›🌍› at offset 0/width 16 and unchanged pane cursor/no file operation. Pair continuation should appear as ››Z› on the second row. Native BE/page-boundary/keyboard/copy/resize variants, physical OS input and VoiceOver remain open. Full cohort parity is not claimed.

Fresh native LE phase-6 capture passed 14/14 checks, all effects zero. Inspected framebuffer shows Cyrillic/globe and second-row ››Z›, original 20 bytes, offset 0, width 16 and independent Hex bytes search mode. Native-launch binary hash verified. CUA returned running-app-not-found after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2868 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
