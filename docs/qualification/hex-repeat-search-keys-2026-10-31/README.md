# Hex repeat-search aliases — 2026-10-31

Independent adaptation of Far Manager Shift+F7 / Space forward and Alt+F7 backward viewer search:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer control keys).

Exact Shift+F7 or unmodified Space repeats forward; exact Alt+F7 repeats backward. These use the existing search worker, query/encoding/Wrap and accepted last-match continuation, without focusing the query. Require no widget focus, popup or active work. Ctrl/Command and combined Alt+Shift variants are excluded. Shared recognition reserves aliases before background pane dispatch so Space cannot change selection and Alt+F7 cannot leak into pane commands. Focused-field F3/Enter behavior remains independent.

Two real-worker/rendered-input regressions cover forward/backward/wrap offsets 0 -> 3 -> 0 -> 3 -> 0, exact bytes, preserved query/bookmark/history, duplicate busy refusal, fields/popups/modifiers and invalid input without queued work.

Dedicated native HexRepeatSearch opens two DE AD BE EF matches at offsets 1/6, focuses/enters the query and runs initial F3, then clicks the non-editable path label to release query focus through actual pointer input. Shift+F7 -> 6, Alt+F7 -> 1, Space -> 6 each require the preceding accepted result. Final capture requires unchanged bytes.bin pane cursor, zero selected files, no operation/modal/copied fixture and accepted result 6. Native Wrap/busy/field variants, physical OS input and VoiceOver remain open.

Fresh native phase-19 capture passed 14/14 checks, all effects zero. Inspected framebuffer shows accepted DE AD BE EF at offset 6, unchanged pane cursor, no selected files or additional dialog. Required checkpoints establish 1 -> 6 -> 1 -> 6 through all aliases after real path-label click. Native-launch binary hash verified. CUA returned running-app-not-found after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2870 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
