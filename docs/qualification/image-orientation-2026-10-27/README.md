# Image preview orientation — 2026-10-27

Independent implementation inspired by the rotation and mirror controls in
[Double Commander's built-in viewer](https://doublecmd.github.io/doc/en/viewer.html).
No upstream source code or new dependencies are included.

## Behavior

Image previews now offer **Rotate left**, **Rotate right**, **Mirror H**,
**Mirror V**, and **Reset orientation**, with explanatory hover text and full
accessible mirror labels. Quarter turns are 90 degrees. Mirrors act in current
display coordinates and compose with earlier rotations. Reset orientation
restores the original texture orientation while retaining zoom. Fit and
double-click retain orientation and reset zoom/position. Changes reset scroll
position; a different source path resets all view state. Each pane is independent.

The image uses the existing texture with permuted UV corners. A rotated image
swaps its display dimensions and requested decode axes, retaining aspect ratio.
No pixel buffer transformation or file-write path was added. Existing bounded
image decoding, memory limits, feature gates and cancellation remain unchanged.
The state is temporary per pane, not saved image edits or EXIF metadata changes.
No new source formats, animation support or performance gain are claimed.

## Verification

Focused tests cover all eight rotation/reflection states, four-turn identity,
inverse rotations, double mirrors, display-space composition, swapped extents,
mesh coordinates and texture binding, path/pane isolation, existing pan, Fit and
zoom limits. The image_preview native scenario now clicks Rotate right then
Mirror H through pointer input and requires the resulting orientation alongside
the existing 200%-zoom and ready-texture checks before capture. It does not seed
the final orientation directly.

The gradient fixture makes the UV transform visible: green increases across the
screen and red down it after the rotation-plus-mirror composition. The resulting
real macOS eframe/Glow framebuffer was visually inspected. Frame, painted glyph,
layout, pane hover and native-effect-isolation results are in manifest.json.
Only this changed native scenario was rerun; this is not a full GUI-matrix or
full-workspace regression claim. The final focused test and Clippy logs accompany
this report.

Final results: **8/8 focused tests**, **14/14 native-frame checks**, Clippy
with all targets/visual-qa and `-D warnings`, formatting and scoped diff checks
passed. No full-suite rerun was needed for this display-only extension.
