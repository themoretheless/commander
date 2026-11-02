# Hex numbered bookmark shortcuts — 2026-10-30

Independent adaptation of [Far Manager viewer bookmark keys](https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4), cohort #37. Ctrl+Shift+digit saves the loaded page start; Ctrl+digit jumps through the existing guarded reader. Commander's existing slots remain numbered 1–10: digits 1–9 address slots 1–9 and 0 addresses slot 10. Left/right Control are not distinguished; Command/Alt combinations are rejected. Hover hints explain the bindings and mapping.

Successful actions choose their addressed slot. Saving shares the existing no-task/no-error/loaded-page guard and queues no work. Empty slots do not read or change the chosen slot. Busy work and open popups suppress the shortcuts. Jumps preserve query settings and use existing snapshot binding, cancellation and successful-read position-history publication. Reload continues to clear all ten session bookmarks. No persistent bookmarks are introduced.

Two headless input/worker regressions exercise saving and reading every slot with exact offsets/bytes/history, query retention, modifiers, empty slots, error suppression, busy generation preservation and open-popup suppression. The initial input fixture omitted egui's ModifiersChanged event, so its failure log is retained; the corrected fixture supplies both modifier changes and key events.

Native keyboard delivery, macOS Control combinations, menu/focus interaction and hover visibility remain pending because CUA is not exposed in this continuation. No native launch was attempted; earlier captures do not qualify these bindings. No commit or push performed.

Final validation: locked visual-QA suite passed 2840 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source and binary hashes accompany these logs. Native validation remains pending.
