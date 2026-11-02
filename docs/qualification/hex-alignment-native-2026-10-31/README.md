# Dedicated native hex alignment qualification — 2026-10-31

The prior HexSearch composite sequence remains unchanged and unqualified for alignment after its pre-alignment pointer-search timeouts. This independent HexAlignment scenario exercises the actual native viewer using egui key events, without search or private offset mutation. The fixture contains 4097 bytes, value index modulo 256.

Required offsets before each action: 0,1,2,1,0,0,4096,4097,4097. Keys: Ctrl+Right, Ctrl+Right, Ctrl+Left, Ctrl+Left, Ctrl+Left (zero no-op), End, Ctrl+Right (EOF), Ctrl+Right (EOF no-op), Ctrl+Left. Final capture requires offset 4096, width 16, unchanged bytes.bin pane cursor, no pending operation/modal/copied fixture. Each action waits for the prior accepted page. Regular unit tests provide exact byte and busy/focus/popup/modifier evidence.

This is an additional scoped scenario, not a substitute for or relaxation of the prior search checks. Physical OS input, VoiceOver and the composite HexSearch alignment chain remain open.

Initial dedicated run preserved under initial-pane-focus: phase 3 stayed at offset 0 with pane-row focus, which correctly suppresses the shortcut. The harness now explicitly clears widget focus before each action to set the documented shortcut precondition. Automatic focus ownership after pane-command opening remains unqualified by this scenario.

Fresh rerun passed all 14 capture checks, zero native effects. Inspected frame.png: offset 0x1000 (4096), 99.97%, one shown byte 00, row width 16, unchanged bytes.bin cursor. Binary hash matches native-launch.json. The CUA observation returned cannotComplete after the short-lived successful capture; fresh manifest establishes completion.

Final source checks: 2856 passed, 0 failed, 3 ignored; Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks passed.
