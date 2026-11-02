# Keyboard background tabs from Find — 2026-10-28

Keyboard follow-up to the Double Commander-inspired [background result-tab transfer](../search-result-background-tabs-2026-10-28/README.md). Cmd+Enter is Commander’s binding, not a claim about Double Commander’s default shortcut.

With the query editor focused, Up/Down selects a visible result and Cmd+Enter reveals it in a background tab while retaining Search and query focus. No selected visible result means no action (no implicit search or hidden-row activation). Ordinary Enter keeps its reveal/start behavior. The shared target lookup resolves displayed time order and archive containers; existing tab caps and workspace guards remain in the background-tab owner.

Native testing reproduced an existing focus defect: consuming ArrowDown selected a result but left egui’s queued directional focus move active. Requesting focus alone did not cancel it. The fix follows the file-list owner’s established pattern: request editor focus and clear FocusDirection::None after Up/Down. The editor also has an explicit stable ID. Rejected logs show focus moving to another widget, then disappearing; final logs keep find-query-editor focused throughout Down/release/Cmd+Enter/release.

The native scenario extends hide/restore/mouse-tab qualification with editor click, real native-window focus precondition, injected ArrowDown and command-Enter press/release. Capture requires three tabs (one original, one mouse-created, one keyboard-created), original active index, selected visible row zero, query focus, unchanged hits and the intended hidden location. It waits for actual OS focus rather than faking WindowFocused. A 30-second Search QA allowance supports activation via the native app tool; other scenario timeouts remain unchanged. Restore clicks have at most two state-checked retries for unchanged GUI state.

Native macOS app-bundle route: **16/16 checks passed**, all recorded native port counters zero. The 1280×760 logical / 2560×1520 frame was inspected: query focus ring, selected notes.md, three tabs and open Search are visible. This qualifies the injected native input/rendering path; it does not claim independent physical-keyboard or VoiceOver attestation. The earlier raw CLI startup limitation is unchanged.

Rejected evidence includes the changed RawInput modifier API (corrected to ModifiersChanged events), a QA-only nested Context-lock panic (focus is now read before writing probe data), an unchanged restore click, inactive-window keyboard input, and the directional focus defect. No rejected result was discarded. The focused regression covers hidden/all-hidden/out-of-range rows, time pivots and archive-container targets. No commit or push.

Final validation: full locked visual-qa suite **2514 passed, 0 failed, 3 ignored**; focused target regression passed; all-target visual-qa Clippy with warnings denied passed. Formatting and scoped whitespace checks passed. The QA app exited after capture.
