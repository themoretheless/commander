# Visible symbolic-link selection — 2026-10-31

Independent adaptation of [fd's symbolic-link type filter](https://github.com/sharkdp/fd/blob/master/doc/fd.1#L209), cohort #77, to Commander's panel selection owner. The command palette adds:

- **Add visible symbolic links to selection**
- **Remove visible symbolic links from selection**
- **Invert visible symbolic-link selection**

The actions apply only to real rows in the active pane's current filtered view. A link to a file, a link to a directory and a dangling link all qualify by the listing's captured lexical Symlink identity. Regular files, directories, hard links, synthetic parent rows and unknown/unobserved kinds do not become symlinks by target appearance. Hidden/filtered-out choices and selected nonlinks remain intact; the cursor/filter/sort/focus owners remain unchanged. Adding twice is idempotent; inversion twice restores the original selection. Notices report entries actually added/removed, or all matching rows toggled.

No extra stat, link traversal or recursive scan is started by selection. A row changed on disk still has its displayed snapshot classification until Refresh; this is a selection action, not an assertion of current link health or authorization to bypass later mutation guards. The actions neither follow targets nor edit/remove/create files. Add/invert require visible entries; remove requires some visible selection, and can be a zero-change action when that selection contains no links.

## Verification

3041 tests passed, 3 ignored, zero failed. All six checks exit zero: visual-qa build/full tests/all-target Clippy with warnings denied, default check, formatting and diff whitespace.

Three new workspace/real-filesystem regressions cover file/directory/dangling symlinks, exclusion of a hard link and regular file, hidden selected-link preservation through a text filter, selected nonlink preservation, stable cursor, exact notice count, add/remove and reversible inversion; snapshot-only classification followed by changed membership after Refresh; and no-link/empty-view availability and no-op behavior. The commands are asserted read-only. Existing selection, catalog/availability and link-operation tests pass.

The existing native CI `symlink_workflow` scene now executes eight palette invocations over 48 phases. After relative-link creation and both target-copy commands, it selects the real visible link, inverts it off, selects again and removes it from selection, requiring exact selection snapshots of 1/0/1/0 in `selection-3.json` through `selection-6.json`. It then follows the cursor link as before. Native capture passed **14/14** and its framebuffer was inspected. Independent artifact assertions validate every new checkpoint, relative collision target, two recording clipboard strings, target-row focus, unchanged source/occupied payloads and launch/core hashes. The scene remains in CI; source hashes are retained.

Native coverage is the normal visible one-file-link workflow. Mixed target types, hidden choices, stale listing and no-op cases are core-tested; physical OS input/pasteboard, every modal/accessibility/layout combination and non-Unix syscall behavior remain outside this slice. fd CLI compatibility and complete cohort parity are not claimed.
