# Transparency backgrounds for image previews — 2026-10-27

Independent implementation inspired by **Show transparency** in
[Double Commander's built-in viewer](https://doublecmd.github.io/doc/en/viewer.html).
No upstream code or new dependencies are included.

## Behavior

The image preview defaults to a neutral gray checkerboard beneath transparent
pixels. **White** and **Black** options help inspect dark/light artwork and
semi-transparent edges. The background changes only the preview composition:
source image files, image alpha and the decoded source texture are not changed.
Opaque pixels continue to cover the background.

Background choice belongs to the current per-pane image view. It survives Fit,
zoom, rotation, mirroring and pan for the same source path; a different path
starts with the checkerboard. The other pane has its own choice. This choice is
not persisted across restarts. Existing image decoding limits and feature gates
remain unchanged. There is no export or alpha-flattening operation.

The checkerboard uses one shared context-cached 2×2 opaque texture, nearest
sampling and repeat wrapping. A single quad covers the image rectangle, clipped
by the existing pane/scroll painter. Its check size is fixed in logical screen
units, independent of image zoom. Zoomed images do not generate a tile for every
square or allocate a large checkerboard bitmap. Solid backgrounds use a filled
rectangle. This is an allocation/geometry bound, not a measured speedup claim.

## Verification

Focused tests cover the repeated texture's dimensions, exact pixels, sampler,
reuse without another upload, constant quad geometry for a huge logical image,
and per-path/per-pane choice. Existing image orientation, Fit, pan and zoom tests
are retained. Initial test attempts used an outdated texture-delta API and
expected egui Image painting to emit a mesh directly; those failed logs are
retained under `rejected/`. The final implementation emits its checker quad
explicitly and test outputs clear their texture deltas before disposal.

The native image_preview fixture now contains opaque, half-transparent and fully
transparent regions. Pointer input rotates it, mirrors it, chooses Black, checks
that Black actually became active, and returns to Checkerboard. Readiness also
requires the composed orientation, 200% zoom and a loaded texture. The captured
macOS eframe/Glow framebuffer is visually inspected for the checker pattern and
partial-alpha blending. The framebuffer manifest records the existing painted
glyph, layout, pane-hover and native-effect-isolation checks. No full GUI matrix
or full-workspace test rerun is claimed for this preview-only change.

Final results: **9/9 focused tests**, **14/14 native frame checks**, Clippy
with all targets/visual-qa and `-D warnings`, formatting and scoped diff checks passed.

The accepted frame includes an incidental “no match” key-sequence toast at the
lower left edge of the preview. It does not cover the background controls or
the checker/partial-alpha regions; it is retained rather than recapturing for
cosmetic cleanliness. No pixel-color accuracy or alpha-math benchmark is claimed.
