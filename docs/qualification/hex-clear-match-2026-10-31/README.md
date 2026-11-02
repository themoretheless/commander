# Clear hex search highlight — 2026-10-31

Independent adaptation of Far Manager viewer Ctrl+U unselect:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer control keys).

Copy match / Clear match and exact Ctrl+U remove search-result highlighting and disable match export without changing the loaded page, query, history or bookmarks. Search continuation is retained separately from visual selection so Find next advances beyond the previous match after clearing, rather than finding the same match again. A newly accepted search reselects the result. This applies to search-created highlights, not arbitrary manual text/byte selection.

Keyboard use requires no busy work, popup or widget focus; Alt/Shift/macOS Command combinations retain ownership. Shared key recognition reserves it before background pane dispatch. The menu uses the same clear method. Clearing does not queue a worker or write to clipboard.

A real-worker/rendered-input regression covers focused fields, popup and modifier guards, preserved page/query/bookmarks/history, no I/O, busy work and next-result continuation. The prior rendered Copy match test now also clicks Clear match and verifies disabled export with retained continuation.

Native HexMatchCopy retains its original exact clipboard/offset/pane checks as a checkpoint, then clicks the menu again and Clear match. Final capture requires unselected result at offset 1, retained pane cursor, no pending file operation/modal and still exactly one recording clipboard write. Physical OS delivery, VoiceOver and native keyboard/guard variants remain open.

Fresh native phase-19 capture passed 14/14 checks. Original copy acceptance remains a required checkpoint; final inspected frame has no match highlight, Copy match disabled, unchanged DE AD BE EF query and offset 1. Exactly one recording clipboard write remains, other effects zero. Native-launch hash matches the built artifact. CUA returned cannotComplete after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2860 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
