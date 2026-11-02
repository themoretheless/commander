# Keyboard copy of a hex match — 2026-10-31

Independent adaptation of Far Manager Ctrl+C / Ctrl+Insert viewer selection copy:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer control keys).

Ctrl+C, Cmd+C, Ctrl+Insert and semantic egui Copy events export the selected found bytes as HEX from the accepted page snapshot, independent of the displayed decimal/HEX mode. The existing validated match slice supplies the data; no new reads/history changes. A key plus Copy event in one frame yields one returned clipboard request.

Fields, popups and real manual label text selection retain copy ownership. Busy/error/missing/cleared matches do not export. Modified Shift/Alt and mixed Ctrl+Command shortcuts are excluded. Shared input recognition reserves eligible events before background panel commands; label selection still receives its event through egui.

Two regressions cover all platform/event forms and duplicate delivery, modifier/focus/popup/cleared/busy guards, exact FF 41 bytes, unchanged position/no workers, and actual pointer-drag selection using egui's label selection plugin.

Native HexMatchCopy keeps the original menu-copy and menu-clear acceptance checkpoints, then clicks Find first and sends Ctrl+Insert. Final state requires selected result at offset 1, unchanged pane cursor/no operation modal, and exactly two recording clipboard writes with DE AD BE EF. The system clipboard is isolated. Physical OS keys, native Cmd+C/semantic-copy/manual-selection variants, VoiceOver and arbitrary byte-range selection remain open.

Fresh native phase-25 capture passed 14/14 checks. Original menu-copy and clear checkpoints retained; Ctrl+Insert created exactly one additional recording clipboard write, final DE AD BE EF and selected result at offset 1 with unchanged pane cursor. Other effects zero. Final frame inspected and native-launch binary hash verified. CUA returned cannotComplete after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2862 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
