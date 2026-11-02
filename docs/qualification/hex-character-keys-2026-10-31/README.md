# Hex character encoding keys — 2026-10-31

Independent adaptation of Far Manager F8 switching and Shift+F8 code-page menu:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer control keys).

Commander F8 cycles its nine supported character encodings in Characters menu order, rather than Far's ANSI/OEM pair. Shift+F8 opens that same menu. Both affect display/Copy page only, including while work is busy, without changing query/search mode, raw data, position/history/bookmarks or queueing work. Focused fields retain text/focus during cycling. Existing popups suppress switching. Ctrl/Alt/Command variants retain their routes. Shared recognition reserves unmodified/Shift F8 before pane dispatch, including busy/field/menu states, preventing background Delete.

Two new regressions cycle all modes plus wraparound with a real focused field, cover popup/modifier/busy guards and preserved state, and open the menu via Shift+F8 then select UTF-8 with pointer events. The initial focus test requested focus before the new window had been rendered; its failure is retained in initial-test-focus-setup.log. Final regression renders the actual window first and uses a defined visible viewport; 80 hex-preview tests pass.

Dedicated native HexCharacterKeys opens CP1251 bytes, sends F8 and requires Windows-1251 / Привет before proceeding, sends Shift+F8, requires an open popup, then selects ASCII. Final capture requires ASCII/dots, unchanged byte snapshot and pane cursor, no pending operation/modal, and all native effects zero. Physical OS input, native focused-field/busy/full-cycle variants and VoiceOver remain open.

Fresh native phase-9 capture passed 14/14 checks, all effects zero. Required intermediate checkpoint proves F8 produced Windows-1251/Привет, Shift+F8 opened the popup and ASCII was selected. Final framebuffer inspected: original bytes, ASCII dots, unchanged pane cursor and no deletion modal. Native-launch binary hash verified. CUA returned running-app-not-found after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2872 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
