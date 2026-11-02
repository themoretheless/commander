# Hex close and pane focus — native qualification, 2026-10-31

Fresh CUA launch of the locally built native visual-qa application. The scenario injects egui input inside the actual application; this is not physical OS-key qualification.

HexSearch retains its former phase-144 acceptance conditions as an explicit checkpoint (UTF-16BE search, wrap, offset 110001, slot 10, width 16, query focus, unchanged bytes.bin pane cursor, no operation modal or copied fixture). Only then does it click Recent searches, require an open popup, press Escape, require popup closed with viewer still present, and press F10. Phase 153 capture requires no viewer or popup, no pending operation/modal/copied fixture, unchanged bytes.bin cursor and actual keyboard focus equal to the left pane's remembered row ID.

The fresh run captured successfully: 14/14 framebuffer/probe checks passed, all native effect counters zero. Inspected frame.png: viewer absent, bytes.bin row visibly focused, two-pane layout retained. The initial CUA AX observation returned cannotComplete after the short-lived application exited; fresh capture artifacts and source-bound readiness gates establish completion. Binary hash matches native-launch.json. Source hashes and command logs are retained here.

Remaining: physical OS Esc/F10 delivery, Escape closing the viewer itself in a native run, native busy-read/search close variants, VoiceOver. The previous source-only report remains historical. Broader 99-repository parity remains open.

Final checks: 2855 passed, 0 failed, 3 ignored; Clippy with warnings denied, default check, visual-qa build, formatting and diff whitespace checks passed.
