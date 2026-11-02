# Hex byte alignment — 2026-10-31

Independent adaptation of Ctrl+Left/Right in Far Manager hex/dump view:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Viewer control keys).

Ctrl+Right loads a page starting one byte later; Ctrl+Left loads one byte earlier, shifting row grouping while retaining configured row width. Existing snapshot validation, bounded 4 KiB worker, position history and cancellation remain authoritative. EOF is a valid empty page; moving before zero or past EOF does not start work. Alt, Shift and macOS Command combinations do not invoke alignment. A focused widget, open popup or busy worker retains ownership. Shared navigation-key detection reserves these keys before background panel dispatch.

A rendered-egui deterministic-worker regression covers exact bytes at every offset of ABCDE, EOF and zero edges, unchanged row width/history, duplicate busy refusal and focus/popup/modifier guards.

Native HexSearch retains its phase-144 checkpoint, explicitly surrenders field focus in the harness, drives Ctrl+Right 110001 -> 110002 and Ctrl+Left back to 110001, then runs the prior menu-Escape/F10/pane-refocus checks. The harness focus setup is not physical pointer or OS-key qualification. Native capture and final check results are recorded separately below.

Remaining: physical OS delivery, VoiceOver, native busy/error/cancellation and empty-file variants; full cohort parity remains open.

The first native attempt timed out at the pre-existing phase-7 search checkpoint after adding a long header hint. That attempt is retained under initial-header-overflow. The alignment hint was moved to the row-width tooltip to retain the prior compact layout before rerunning; the readiness predicates were not weakened.

Second native attempt progressed to phase 24 but timed out in the existing UTF-8 pointer-search sequence, before alignment. Retained under second-search-pointer-timeout. Native alignment remains unverified; no failed capture is counted as success. The full suite initially exposed an older row-width guard test leaving the newly meaningful Ctrl+Right read queued; that test now finishes the read and asserts offset 1 while retaining its unchanged-width contract.

Final validation: 2856 passed, 0 failed, 3 ignored. Clippy with warnings denied, default cargo check, visual-qa build, formatting and diff whitespace checks all passed. Native alignment remains unverified.
