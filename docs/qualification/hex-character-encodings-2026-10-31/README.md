# Hex single-byte character encodings — 2026-10-31

Independent adaptation of Far Manager's encoding-aware hex character column:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer mode / hex mode).

Characters offers ASCII (existing default), Windows-1251, Windows-1252, KOI8-R, KOI8-U and IBM866. Only the character column and Copy page change. Hex/decimal byte values, absolute offsets, row width, raw byte exports, strict UTF-8 export and search encoding remain independent. Each source byte yields one displayed character. Controls, undefined bytes, soft hyphen and non-ASCII whitespace become dots. Lazy per-encoding lookup tables avoid decoding/allocating per displayed byte after initialization. No performance claim is made.

Two regressions cover known Russian, Ukrainian and Latin fixtures, all 256 bytes in all six modes and widths 1/17/64 with both byte formats, unchanged raw columns and character counts, control/replacement suppression, plus actual selector/Copy page pointer actions, unchanged search/history/bookmark/generation, no queued work and untouched backing file.

Dedicated native HexCharacterEncoding opens six CP1251 bytes CF F0 E8 E2 E5 F2 through the pane command, clicks Characters / Windows-1251, and requires visible first row ending in Привет, offset 0, width 16, unchanged bytes.bin pane cursor and no file operation/modal/copied fixture. No system clipboard action is used in this scenario.

Remaining: physical OS input, VoiceOver, native other-codepage/Copy-page/resize variants and UTF-8/UTF-16 character-column rendering. Full Far viewer or cohort parity is not claimed.

Fresh native phase-6 capture passed 14/14 checks, all effect counters zero. Inspected framebuffer shows Characters: Windows-1251, original CF F0 E8 E2 E5 F2, readable Привет, unchanged offset 0/width 16 and Hex bytes search mode. Native-launch binary hash verified. CUA returned running-app-not-found after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2864 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
