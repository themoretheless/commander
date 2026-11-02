# Remove character ranges during batch rename — 2026-10-30

Independent adaptation of [Thunar Remove Characters](https://docs.xfce.org/xfce/thunar/bulk-renamer/start), repository #24 in the cohort. No upstream implementation copied. Commander exposes skip/count/from-end controls for the stem; this is not the full Thunar renamer/plugin interface.

Remove characters defaults off. When enabled, Skip retains an offset of grapheme clusters at the chosen edge and Remove deletes the following count towards the opposite edge. UI and presets support offset 0–255 and count 1–255. Short names clamp instead of wrapping; saturating arithmetic also handles arbitrary direct-core usize inputs. Combining sequences and emoji ZWJ clusters stay whole. Files keep the extension after find/replace; directory names are whole stems. Dotfiles follow the existing extension split.

The operation runs after all replacement pairs and before case, prefix/suffix and numbering. Existing whitespace/normalization/final-length cleanup follows. Empty or colliding final names use the existing validation and filesystem collision checks. RenameRule serde defaults preserve older presets and omit inactive removal. Presets validate supported bounds, retain the chosen edge/range, and Reset disables/removes the range. Cache equality includes all range fields.

Six new regressions cover both edges, Unicode, huge offsets/counts, short names, dotfiles/directories/multiple extensions, transform order, legacy serde, empty/colliding results, supported/invalid preset bounds, cached submit rules and real payload/Undo/Redo/sibling collision refusal. The existing reset regression now also loads a range rule and verifies UI roundtrip/reset.

Native RenameOptions retains its earlier workflow. Before two-target numbering it clicks Remove characters, requires Final.txt→inal.txt, clicks from end and requires Fina.txt, then disables removal and requires Final.txt again. All later ordering/exclusion/clipboard tests remain enabled; final readiness also requires completion of the removal interaction stages. No rename is submitted by this UI scenario; actual filesystem behavior is exercised by the workspace regression.

Full locked visual-qa suite: **2628 passed, 0 failed, 3 ignored**. All-target Clippy with warnings denied, locked visual-qa build, cargo fmt --check and git diff --check passed. Fresh CUA-launched native capture passed **16/16**, with removal phase 12 and overall phase 97 required by readiness. Exactly three recording clipboard calls from the retained export tests, all other native effects zero. Manifest postdates native-launch.json. The native application captured and exited normally. No commit or push; the overall transfer objective remains open.

Frame visually inspected: the removal row and skip/count/from-end controls fit within the modal. Removal is disabled in the final frame after the required start/end checks; the retained notes.md→01.md preview and export controls remain visible.
