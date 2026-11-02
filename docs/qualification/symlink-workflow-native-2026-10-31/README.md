# Native palette workflow for symbolic links — 2026-10-31

Closes the normal native command-delivery gate for the relative-link creation, target-copy and Follow adaptations of [Yazi link/follow](https://yazi-rs.github.io/docs/configuration/keymap/), cohort #27, and Commander's target-copy extension. Production command semantics are unchanged in this milestone; a new native `symlink_workflow` scenario and CI matrix entry exercise them together.

The scene starts with `left/portable.txt` containing `portable source\n` and an occupied `right/portable.txt` containing `occupied destination\n`. Through actual egui Command+K, typed command-palette queries and Enter events it runs four commands, requiring the selected first result to be the intended enabled command:

1. Create relative symlink in other panel: a real `right/portable link 2.txt` must point to `../left/portable.txt`, preserving the occupied file.
2. Copy symbolic-link target: the recording clipboard must receive exactly `../left/portable.txt`.
3. Copy resolved symbolic-link target: its second write must equal the physical absolute source path.
4. Follow symbolic link: the right pane must navigate to the source's resolved parent and its actual cursor must land on `portable.txt`; the left root remains unchanged.

The native fixture uses real symlink creation, listing, path resolution and navigation. Clipboard writes use the harness's existing recording ClipboardPort; no host pasteboard, opener, trash, native menu or free-space effects are requested. Source/link/occupied-file contents are checked before fixture cleanup. Filesystem checks after application exit are not inferred from deleted fixtures.

## Verification

3038 tests passed, 3 ignored, zero failed; all six checks exit zero: visual-qa build/full suite/all-target Clippy with warnings denied, default check, formatting and diff whitespace. Scenario enumeration now contains 64 stable names; the mirrored clipboard-count contract requires exactly two writes for the new scene. The scene is in the existing native CI matrix.

Native capture passed **14/14** checks at 1280×760, inspected in `native/frame.png`. Independent `verify-artifacts.py` assertions validate the completed 24-phase workflow, exact two clipboard strings, relative collision target, active pane/cursor/resolved parent, unchanged source/occupied payloads and matching launch binary/core hashes. Qualified source hashes are retained.

The first native attempt is preserved in `before-first-native/`: it timed out with zero clipboard writes because the test supplied Command only inside the key event, while the generic command mapper reads the global InputState modifiers. The corrected generator emits egui 0.36 `ModifiersChanged` events as well as key events; the repeat completes all four palette commands. A readback-expression compilation fix and stable-scenario-list update are also retained in the qualification logs. These were harness corrections; no production command dispatch was changed to force the capture.

This qualifies normal palette delivery, one real file link with a collision, the two recording clipboard payloads and target navigation/focus. Physical OS key delivery, host pasteboard readback, every popup/layout/accessibility configuration, folders/tree moves, broken/loop/non-UTF cases remain outside this native slice; the existing filesystem/core tests cover their documented subsets. Full upstream/cohort parity remains open.
