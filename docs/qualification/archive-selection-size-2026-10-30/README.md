# Archive selection size summary — 2026-10-30

Independent adaptation of [Ark InfoPanel selected-size summary](https://github.com/KDE/ark/blob/master/part/infopanel.cpp), cohort #99. Verified upstream setIndexes sums size metadata across multiple selected entries. Commander independently adds explicit record-type counts and overflow-safe integer totals.

The archive pane now shows Selected file/directory/symbolic-link/other counts and exact unpacked byte totals for selected regular files. ZIP also shows packed byte totals for those regular files; TAR reports packed size unavailable rather than zero. Zero is a valid known ZIP packed total. Totals include hidden/cross-folder/type-filtered/narrowed-out choices, and duplicate member records count separately. Directory, symlink and other record sizes do not enter regular-file byte totals; selecting a real directory entry does not implicitly select its subtree. Existing explicit subtree selection still contributes each selected descendant normally. Synthetic/stale IDs are ignored.

Metadata is declared by the loaded listing and is not verified payload length, required disk allocation, extraction success or current source identity. All sums use u128, preserving values beyond u64 and floating-point precision. No new archive reads, extraction or clipboard writes occur. One per-listing cache compares the sorted selection key and recomputes only when it changes; repeated repaints do not rebuild the snapshot or rescan metadata. Key equality remains linear in selection count. Listing replacement/close resets it. The label uses current selection after selection controls so Save/Restore, clear and inversion are reflected on the same frame.

Two regressions cover duplicate identities, counts, stale IDs, ignored nonregular sizes, exact totals above u64, known ZIP zero versus unavailable TAR values, actual folder/filter hiding and cache replacement/empty/reset behavior.

Native summary layout and selected-size workflow remain pending because desktop CUA is unavailable. Earlier archive native gates remain open. No native capture success is claimed.

Validation:

- Initial locked visual-qa full suite: **2762 passed, 3 ignored, zero failed** (`test.log`).
- Final-source full rerun: **2762 passed, 3 ignored, zero failed**, after adding an explicit known-zero packed-size assertion; recorded separately (`test-final.log`).
- All-target visual-qa Clippy with warnings denied, default locked check and visual-qa build passed (`clippy.log`, `check.log`, `build.log`).
- Formatting and diff whitespace logs accompany this report.

Formatting and whitespace checks passed. Source/binary hashes and the pending native-validation record accompany this report.
