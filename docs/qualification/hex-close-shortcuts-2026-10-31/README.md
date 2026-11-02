# Hex viewer close shortcuts — 2026-10-31

Independent adaptation of Esc/F10 viewer close from Far Manager (cohort #37):
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4

Unmodified Esc/F10 are reserved before panel keyboard dispatch. The viewer checks popup ownership before closing, consumes the close keys, cancels its owned task and returns closed before starting an initial read. The application drops the viewer, surrenders stale widget focus and requests the existing bounded cursor-row refocus. Mouse close uses the same application cleanup.

Two rendered-egui regressions cover both keys, initial and queued reads, cancellation/no late page publication, popup precedence and Control-modified keys. Existing cursor-row refocus tests remain part of the full suite. See checks.json and individual logs for final command outcomes.

Native application close/focus, real OS key delivery, VoiceOver and busy-search native variants have not been qualified in this milestone. Earlier native HexSearch captures predate this change and are not close-shortcut evidence. The broader 99-repository transfer remains open.

Final validation: 2855 passed, 0 failed, 3 ignored. Clippy with warnings denied, default cargo check, visual-qa build, cargo fmt --check and git diff --check all passed.
