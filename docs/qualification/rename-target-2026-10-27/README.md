# Targeted find/replace in batch rename — 2026-10-27

Independent adaptation inspired by the extension inclusion option and separate
name/extension handling in [Double Commander's Multi-Rename Tool](https://doublecmd.github.io/doc/en/multirename.html).
No upstream application code or new dependency was added.

## Behavior

The existing batch-rename studio now offers **Apply find/replace to**:

- **Whole name** retains the previous behavior, including extension replacement.
- **Name stem** transforms the original stem and retains the original last
  extension exactly, even if the replacement itself introduces dots.
- **Extension** transforms the original last extension without its separator dot.

All modes use the existing literal or regex replacement and capture expansion.
Case, prefix, suffix and numbering still run afterward on the resulting stem.
This selector affects find/replace only, not those other edits. Folder names are
whole stems; Extension mode skips their find/replace, but other configured stem
edits still apply. The inline explanation makes this distinction visible.

The split uses the last interior dot: archive.tar.gz has stem archive.tar and
extension gz; a leading-only dot in .gitignore is part of its stem. An unchanged
empty extension preserves the original spelling, including a trailing dot.
Replacing a nonempty extension with empty text removes its separator. A regex
such as ^$ can add an extension to an extensionless file. The UI preview remains
the authority before Rename; leading dots supplied inside replacement text are
not silently removed or normalized.

## Planning, persistence and application

The target mode is part of RenameRule equality, so changing it invalidates the
cached plan and submit eligibility. A stem-only no-op cannot submit an earlier
whole-name plan. Interacting with the selector participates in the existing
Enter guard used by preset controls. Changing the option only updates preview;
it never applies a filesystem rename itself.

Saving/updating a preset stores the mode; loading it restores that mode with the
other fields. Existing rules missing the new field default to Whole name, whose
default field is omitted on serialization to preserve the previous JSON shape.
Unknown mode values fail deserialization rather than silently changing scope.
Presets remain Commander's own format, not a Double Commander import format.

The existing captured-target context, name policy, collision/invalid-name checks,
rename ordering, rollback, Undo and Redo are reused. Application does not rewrite
file content. This change does not add recursive rename, extension-case toggles,
new placeholder syntax or new numbering semantics.

## Verification

- Initial default rename-filter run: 119 passed.
- Expanded rename-filter run with visual-qa: **120 passed**.
- Full regression after the implementation and native-harness correction:
  **2320 passed, 3 ignored**.
- Native compilation initially caught a test-only selection helper used in the
  new scenario. It was replaced with the production selection API; failed
  native/Clippy build logs are retained in rejected/.
- Visual inspection found partially clipped action buttons under the sheet's
  default initial height. The sheet now requests a viewport-relative initial
  height, retaining its bounded outer scrolling for smaller windows. After this
  display-only adjustment, **15/15 batch-dialog tests**, all-targets/visual-qa
  Clippy with -D warnings, formatting and scoped Git whitespace checks passed.
  The full suite was not repeated solely for that height adjustment.

Pure tests distinguish all three targets, inserted dots, extension removal and
creation, dotfiles/trailing dots/compound names, directory behavior, regex groups,
structural ordering, collisions, invalid components and legacy/unknown JSON.
Preset tests round-trip every mode. A workspace test applies extension-only
renaming to real temporary files, verifies unchanged content, Undo/Redo and
collision refusal without touching the occupied target. A dialog test ensures
scope changes rebuild the cache and block stale no-op submission.

The native rename_target scenario opens a real fixture selection, uses pointer
input to select Name stem and waits for its cached no-op preview, then selects
Extension and requires item.txt -> item.log in the actual cached plan. The source
fixture must still exist and the destination must not exist, proving that merely
choosing a scope did not apply it. The final macOS eframe/Glow frame was visually
inspected, including the fully visible Rename/Cancel buttons, and all manifest
checks passed. The CI/README matrix now includes 19 cases; no remote CI or push
is claimed. No new performance claim is made.
