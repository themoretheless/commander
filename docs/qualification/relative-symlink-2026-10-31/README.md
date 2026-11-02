# Relative symlinks into the other pane — 2026-10-31

Independent adaptation of [Yazi `symlink --relative`](https://yazi-rs.github.io/docs/configuration/keymap/#symlink), cohort #27. The command palette adds **Create relative symlink in other panel**, using the existing active selection/cursor, inactive destination, mutation guards, capability requirements, collision naming, pane refresh and aggregate notice owners. The previous absolute-symlink and hard-link commands retain their behavior. No external `ln` invocation or dependency is added.

The new command writes a relative target rather than an absolute path. It resolves the source's containing directory and the destination containing directory physically before computing `..` components, so opening panes through symlinked folders does not produce an incorrectly anchored target. The source's final filename is preserved, including a symlink or dangling symlink; its final referent is not canonicalized. Path components use native OS strings, without lossy display-name conversion. A shared-root mismatch or failed containing-directory resolution returns an item error through the existing partial-result report.

Files and folders are supported. Existing destination entries remain intact; collisions use the existing `name link 2.ext` scheme. Moving the common directory tree preserves links when the relative structure remains the same; arbitrary independent moves or symlink retargeting have their ordinary filesystem semantics. Unix hosts use the existing native symlink syscall owner, with the existing unsupported-host error elsewhere. Creation retains the original synchronous, path-based link operation semantics; no new transaction, undo journal or concurrent-parent-replacement guarantee is claimed.

## Verification

3030 tests passed, 3 ignored, zero failed. All six checks exit zero: visual-qa build/full suite/all-target Clippy with warnings denied, default check, formatting and diff whitespace.

Three new regressions execute real native filesystem operations. The routed workspace command links both a selected file and directory, preserves an occupied filename, checks exact relative target bytes and the two-link notice, then renames the common tree and reads both targets successfully before restoring the fixture. A physical-parent regression opens the destination through a directory symlink, requires `../../source/file`, reads the payload, and verifies a link to an existing dangling symlink remains a link to that lexical entry. The mutation-block regression refuses creation with the existing integrity-review block. The command joins the existing mutating-command classification assertion; catalog uniqueness, availability and shortcut-hint checks cover the added registry entry. Existing absolute-link/hard-link tests remain green.

These are actual host filesystem and command-routing tests, not a new native GUI capture. Palette keyboard delivery/layout and every capability/provider/non-Unix combination remain unqualified for this action. Full cohort parity remains open.

## Native workflow follow-up

A later [native palette scenario](../symlink-workflow-native-2026-10-31/README.md) passed 14/14, covering real relative-file-link creation with a collision, both recording clipboard payloads and Follow target-row focus. This supersedes the normal palette-delivery pending boundary only for that workflow; the report specifies remaining variants and OS pasteboard/input limits.
