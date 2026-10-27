# Sort by file name without extension — 2026-10-28

Independent adaptation of [Double Commander UniversalSingleDirectSort column=namenoext](https://doublecmd.github.io/doc/en/cmds.html), which its documentation marks as an alpha option. No upstream code copied; no broad implementation parity claim.

The palette offers Sort by name without extension. The new Stem sort column compares files before their final extension; dotted directory names remain whole. Dotfiles and multi-extension names use standard Path file_stem semantics. Existing normalized case/diacritic keys and natural/plain ordering apply without allocating new keys inside the comparison. Folders-first remains independent. Equal stems break ties by ascending full-name order, including in descending mode, matching existing non-name sort behavior. Repeating the command reverses direction.

The Name (no extension) header carries the current direction and reverses this mode when clicked. Sort by name returns to full-name ordering. The new enum value travels through existing view/session/tab serialization; older configuration values remain unchanged. Existing cursor/selection reconciliation is reused.

Verification:

- Full local suite: **2429 passed, 0 failed, 3 ignored**. Sort tests cover ascending/descending stems, equal-stem tie direction, dotfiles, final-extension-only handling, dotted directories and natural/plain numeric order. Both-pane command regression proves actual before/after row order, repeated-command direction, cursor/selection retention, untouched other-pane view and serialized tab-view roundtrip.
- Initial **2 targeted sort tests passed**. All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

No native interaction/frame capture or desktop restart was run for this new header/mode; serializer and dispatch assertions do not prove a real process restart. Remote CI and other native scenarios were not rerun. No commit or push.
