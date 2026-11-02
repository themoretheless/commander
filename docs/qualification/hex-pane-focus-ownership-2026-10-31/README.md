# Hex viewer pane-focus ownership — 2026-10-31

Native alignment testing exposed remembered pane-row focus after opening the viewer through the pane command. The navigation shortcuts intentionally require no widget focus, so Ctrl+Right was correctly suppressed despite the foreground viewer.

The application now retires deferred pane-refocus requests and releases only remembered left/right pane-row keyboard focus while the hex viewer is present, before keyboard dispatch. Opening also retires pending requests. Viewer text fields and popup owners retain focus. Closing continues to request cursor-row focus through the existing bounded mechanism.

A rendered file-list regression establishes actual row focus, queues a deferred request, releases ownership, renders four more frames to prove no reassertion, verifies query/popup focus preservation and right-pane release. Dedicated HexAlignment no longer clears focus in its harness: the native command-open -> Ctrl-arrow path exercises application behavior directly. Historical focus-clearing captures remain historical and are not used as this fix's evidence.

Remaining: physical OS input, VoiceOver, other viewer types and the older composite pointer-search scenario.

Fresh native HexAlignment captured phase 27 with 14/14 checks and zero effects, without harness focus clearing. Inspected final framebuffer: offset 4096, one byte 00, width 16, pane cursor retained. Native-launch SHA matches the built artifact. CUA AX observation returned cannotComplete after the successful short-lived capture; fresh output provides completion evidence.

Final validation: 2857 passed, 0 failed, 3 ignored; Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
