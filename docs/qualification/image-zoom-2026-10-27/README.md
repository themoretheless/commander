# Interactive image preview qualification — 2026-10-27

Reference: [Kiorg image preview](https://github.com/houqp/kiorg/blob/main/crates/kiorg/src/ui/preview/image.rs).
Original implementation; no upstream Rust source was copied.

## Verified behavior

`cargo test preview -- --test-threads=1`: **75 passed**, no failures. New cases
check aspect-preserving Fit geometry, explicit relative magnification, zoom bounds
and invalid gestures, independent panes/path reset, actual synthetic pointer drag
producing a nonzero scroll offset, and actual double-click returning zoom to Fit.
Existing text/document/table preview and relevant cache target-bound regressions
also pass. The final spacing-only toolbar adjustment followed this run.

Native harness tests: **3 passed**, including the ten-case scenario matrix.
Final Clippy across all targets/features with warnings denied, formatting and
whitespace checks pass. Test filters overlap and should not be added as a unique
test total. No benchmark or installed-release/complete VoiceOver qualification.

## Native frame

The new `image_preview` scenario writes a real 960×720 PNG, waits for actual cursor
binding, opens the image preview at 200% of Fit and requires a published texture
and matching zoom state before capture. The retained frame is visually inspected
for image pixels, readable Fit/minus/plus controls, explicit scale label, close
control, clipping and pane geometry. Native-effect recording remains isolated.

The decoded image pipeline remains asynchronous/color-managed and retains its
existing 4096-pixel target cap and memory checks. Enlarging can upscale a bounded
bitmap; the percentage is relative to Fit, not original source resolution. The
viewer does not rotate, edit or save images. Source close-by-click was replaced
by viewer interaction; Close/Esc remain.

## Rejected evidence and corrections

An initial drag test showed no offset with scroll-area background dragging.
The image canvas now owns the click/drag gesture and requests an instantaneous
scroll delta; the pointer test proves movement. Intermediate diagnostic logging
was removed from the implementation. An initial native attempt stayed idle in
the macOS event loop with an attempted-only manifest. Sampling showed the main
thread waiting for native events, not a blocked decoder. That verified live test
process was terminated; its attempted manifest/log/sample are retained under
`rejected`. The harness now requests periodic repaints while captures are pending
so retry/timeout observation cannot stop merely because the UI becomes idle.
Checks and timeout were not weakened. A temporary syntax error while removing
instrumentation was corrected before accepted validation/capture.

Accepted native result: **captured**, all **14** manifest checks passed and all
recording-native-effect counters are zero. [View the frame](image_preview/frame.png).
