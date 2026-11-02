# Hex position-history keyboard commands — 2026-10-30

Independent adaptation of [Far Manager viewer position undo keys](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Ctrl+Z and Alt+Backspace invoke the existing Back history owner. Ctrl+Shift+Z is Commander's companion Forward binding over its already implemented forward stack. Hover hints and README describe all three.

The input owner requires no active work, no open popup and no focused egui widget. Text fields retain undo and word-deletion handling; macOS Command combinations and unrelated modifiers are not taken over. Keyboard commands call request_history through the same dispatch as buttons: snapshot identity, cancellation/generation, successful-read-only stack commits and branch behavior remain shared. No new history storage or direct page changes are introduced.

A headless input/worker regression builds three distinct positions, exercises Ctrl+Z, Alt+Backspace and Ctrl+Shift+Z with exact loaded bytes, rejects duplicate busy work, and covers focused query, popup and Command suppression. The extended HexSearch native scene appends Ctrl+Z from 110001 to 120001 and Ctrl+Shift+Z back to 110001 after its existing width sequence. Each stage waits for its source position; final capture requires phase 123 and preserves the earlier bookmark/mode/width/wrap invariants.

Native results and final checks follow below. Physical OS keyboard delivery, VoiceOver, native text-field undo/popup/busy interactions and Alt+Backspace remain separate gates. No commit or push performed.


Fresh native HexSearch captured successfully with 14/14 checks and all recorded effects zero. The scene reached the exact Back/Forward source/destination gates and final 110001; its inspected framebuffer shows the returned offset, retained slot 10, UTF-16 BE query/wrap and 16-byte layout. This proves native rendering and injected event/worker integration, while the remaining physical-key/focus/error variants stay open. CUA's AXError.cannotComplete followed the own application's successful fast capture/exit; fresh unique artifacts establish completion.

Final verification completed after local date rollover to 2026-10-31 (Asia/Tbilisi): locked visual-QA suite passed 2848 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. The report directory retains its start date. Source/binary hashes accompany the logs and match the fresh native launch.
