# UTF-8 hex character column — 2026-10-31

Independent adaptation of Far Manager UTF-8 hex rendering (leading character, U+203A continuation markers):
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer hex mode).

Characters now offers UTF-8. Valid scalars appear at their leading byte and continuation positions show ›. Rendering validates complete sequences, including overlong/surrogate/out-of-range refusal, and dots invalid bytes. Controls, combining marks and invisible formatting are sanitized rather than changing row layout. Wide glyph advances remain font-dependent; each source byte still has exactly one Unicode scalar in the exported character column.

A page read includes at most three context bytes before and after the public 4096-byte page, through the same handle and before/after stamp validation with cancellation checks. Context is not exported as page data. This preserves valid UTF-8 across page/row boundaries and when jumping into a sequence. Character switching performs no new read. Raw byte offsets/counts, byte export, search encoding and history stay independent. Public page bytes remain unchanged; maximum context read is 4102 bytes.

Two regressions cover four-byte emoji/2-byte Cyrillic and Latin across offset 4095 with widths 1/3/16/64, mid-sequence offsets, EOF and bounded context; malformed overlong/surrogate/out-of-range/truncated/orphan sequences; control/combining/bidi formatting; actual UTF-8 selector and Copy page pointer actions with unchanged generation/query/no workers and exact raw UTF-8 bytes.

Dedicated native HexUtf8Encoding opens AПривет🌍Z, selects Characters / UTF-8 and requires offset 0, width 16, first-row byte-position text AП›р›и›в›е›т›🌍››, unchanged pane cursor and no file operations. The emoji crosses the 16-byte row boundary; the second row starts ›Z. Physical OS input, VoiceOver, native page-boundary/copy/resize variants and UTF-16 column support remain open. Full cohort parity is not claimed.

Fresh native phase-6 capture passed 14/14 checks with all effects zero. Inspected framebuffer shows Characters: UTF-8, readable Cyrillic and globe glyph with continuation markers, second-row ›Z and unchanged original byte columns/offset 0/width 16. Native-launch binary hash verified. CUA returned running-app-not-found after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2866 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
