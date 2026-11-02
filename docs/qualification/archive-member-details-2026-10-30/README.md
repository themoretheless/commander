# Archive member details — 2026-10-30

Independent adaptation of [Ark InfoPanel per-entry metadata](https://github.com/KDE/ark/blob/master/part/infopanel.cpp), cohort #99. Upstream shows per-entry information and executable status. Commander independently exposes already-loaded archive metadata; octal mode display is a Commander extension, not a claim that Ark's InfoPanel uses this exact field layout.

A collapsed-by-default Member details panel appears beneath the existing member-ID/path line. It follows the same real-member cursor/fallback owner: current real cursor, or the first selected member when no cursor exists. Synthetic navigation IDs do not produce fake metadata. Expanding reveals Kind, Declared size, Packed size, Modified (UTC), Unix permissions and Executable. Existing selection, filters, history, scrolling and extraction owners are unchanged.

Exact decimal sizes retain zero. ZIP packed size is shown when applicable; TAR and directories show Unavailable. Directory declared size is marked not applicable rather than treated as implicit subtree size. Modified uses the shared UTC export formatter and does not fall back to the physical archive's mtime. Unix permission bits are octal with at least four digits and special bits retained; file-type bits are masked out. Some(0) produces 0000, false produces No, and absent metadata produces Unavailable. Types distinguish symbolic links, directories, regular files and other archive records. Values are declared metadata, not verified payload bytes, executable-content analysis or the permissions ultimately applied by extraction.

The panel is read-only and formats only when opened. No member payload reads, link following, filesystem/clipboard changes, new parsing or CSV schema changes occur. Missing owner/group/link-target values are not invented, and full Ark properties parity remains open.

Two regressions cover known zero/false versus absent values, epoch UTC, ZIP/TAR packed availability, special permission bits, all record types and directory sizes.

Native expand/cursor/layout verification remains pending because desktop CUA is unavailable. Earlier archive native gates remain open; no native capture success is claimed.

Validation:

- Locked visual-qa full suite: **2768 passed, 3 ignored, zero failed** (`test.log`).
- All-target visual-qa Clippy with warnings denied and default locked check passed (`clippy.log`, `check.log`).
- Formatting and diff whitespace checks passed (`fmt.log`, `diff-check.log`).
- Locked visual-qa build passed (`build.log`). Source/binary hashes and pending native-validation record accompany this report.
