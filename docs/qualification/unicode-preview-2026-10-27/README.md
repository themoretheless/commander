# BOM-marked Unicode text previews — 2026-10-27

Independent implementation inspired by [Double Commander's viewer encoding
support](https://github.com/doublecmd/doublecmd/blob/master/components/viewer/viewercontrol.pas).
The upstream viewer exposes UTF-16LE/BE and UTF-32LE/BE. Commander implements
signature-based decoding in its existing bounded background text-preview path;
no upstream implementation code or new dependencies are included.

## Behavior and boundaries

UTF-16 little/big endian and UTF-32 little/big endian files with a BOM are
converted to UTF-8 for the existing preview UI. Text search, line numbers,
selection, copying and document/table rendering consume the decoded string.
The initial signature is removed; later U+FEFF characters are preserved. CRLF,
tabs, non-ASCII text and supplementary characters retain their Unicode values.
No source file is rewritten.

Detection follows the [Unicode BOM definitions](https://www.unicode.org/faq/utf_bom.html):
four-byte UTF-32 signatures take precedence over UTF-16. Unmarked input follows
the existing UTF-8 path. There is no filename-based encoding choice, heuristic
UTF-16 detection, legacy-codepage fallback or manual encoding menu. These wider
viewer capabilities remain comparative work rather than completed transfers.

Malformed surrogate pairs, odd UTF-16 byte counts, incomplete UTF-32 units,
surrogate UTF-32 values and scalars above U+10FFFF fail with a specific malformed
Unicode error. They are not silently replaced. Decoded NULs and excessive ASCII
controls still trigger the existing binary-data policy. A BOM is an encoding
signature, not permission to treat arbitrary binary content as text.

The existing 256 KiB source-file limit remains, and decoded UTF-8 output also
cannot exceed 256 KiB. Expansion beyond that limit fails explicitly without
publishing partial text. The existing incidental-invalid-UTF-8 replacement path
now checks the same output bound. Files are not truncated to make decoding pass.
The memory cache already accounts for actual decoded-string bytes and retains
its existing total/entry limits. Conversion runs on the existing worker after
its before/after file-fingerprint checks. Cancellation, timeout, pane ownership,
stale-completion rejection, retry behavior and native-effect isolation retain
the existing pipeline. No claim is made about arbitrary encodings or searching
UTF-16 file content through the separate filesystem search engine.

## Verification

Tests exercise four BOM/endian combinations, multilingual and supplementary
characters, empty BOM-only text, embedded U+FEFF, CRLF/tabs, malformed sequences,
binary policy, input/output limits, exact-limit acceptance and replacement-text
expansion. Actual files in each encoding are read through FsTextPreviewProvider,
rather than testing only an isolated string converter. Existing image/text cache,
read-cancellation, identity-change and worker-lifecycle tests also run.

The new native `unicode_preview` scenario creates a real UTF-16BE text file,
opens it through the regular preview action and loader, requires exact decoded
content, and checks two search matches before screenshot capture. It does not
inject the resulting text into preview state. Framebuffer and native-port
isolation results accompany the screenshot. The CI matrix now has 15 scenarios;
no remote CI execution is claimed.

Full test run: **2286 passed, 3 ignored**, no failures. The native scenario
passed **14/14 frame checks** and its screenshot was visually inspected. After
that run, Clippy required replacing constant-size `chunks_exact` iteration with
`as_chunks`; the failed lint log is retained under `rejected/`. Focused tests
and Clippy were rerun after this mechanical iteration change; the full suite
was not repeated.

Final focused run: **49/49 image-cache/text-preview tests passed**. Clippy with
all targets, visual-qa and `-D warnings`, formatting and scoped diff checks passed.
