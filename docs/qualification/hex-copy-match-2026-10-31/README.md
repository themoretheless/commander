# Copy found hex bytes — 2026-10-31

Independent adaptation of Far Manager viewer copying a search-created selection:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer control keys).

Copy match exports only the accepted found byte slice as spaced HEX, three-digit decimal or strict UTF-8. It reads the loaded snapshot, not the file again. UTF-8 preserves whitespace/controls and is disabled for invalid data. Availability requires an idle, error-free page whose offset and prefix match the recorded nonempty search pattern. It rejects missing, incomplete or stale match bindings. Existing full-page export shares the same byte formatter.

Two regressions drive all three menu actions, including a cross-page-boundary result at 4094 and invalid UTF-8 refusal, then verify exact text, unchanged offset/history and no new worker. The backing file is replaced after accepting the result to prove snapshot ownership. Guard regression covers missing/page/busy/cancel/error/wrong-offset/wrong-bytes/empty/overlong bindings.

Dedicated native HexMatchCopy uses a six-byte fixture 00 DE AD BE EF 00, focuses Search with F7, enters DE AD BE EF, invokes F3, requires found offset 1, then clicks Copy match / HEX bytes. Capture requires exactly one recording clipboard write equal to DE AD BE EF and unchanged pane cursor/no file operation. It does not write to the system clipboard.

Remaining: physical OS keys, VoiceOver, native decimal/UTF-8/busy/error variants and arbitrary manual byte-range selection. Full Far viewer or cohort parity is not claimed.

Fresh native capture: phase 13, 14/14 checks, exactly one recording clipboard write equal to DE AD BE EF, all other effects zero. Final framebuffer inspected: found bytes and surrounding 00 visible, Copy match unbroken label, corrected generic copy toast. Prior successful capture with misleading page-only toast is retained under initial-toast-label; final capture matches native-launch binary hash. CUA returned running-app-not-found after the short-lived application exited successfully; fresh capture artifacts establish completion.

Final validation: 2859 passed, 0 failed, 3 ignored. Clippy with warnings denied, default cargo check, native visual-qa build, formatting and diff whitespace checks passed.
