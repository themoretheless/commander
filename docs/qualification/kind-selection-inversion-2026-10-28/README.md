# Invert file/folder selection independently — 2026-10-28

Independent adaptation of [Double Commander’s attribute-scoped inversion](https://doublecmd.github.io/doc/en/cmds.html): cm_MarkInvert uses the marking attribute filter, whose d-/d+ settings distinguish files and directories. Commander exposes two direct palette actions. No upstream code/dependency copied.

Invert visible file selection and Invert visible folder selection toggle only the corresponding entries in the current filtered view. The other kind, filtered-out picks, cursor and opposite pane remain unchanged. Repeating the same inversion restores its previous state. Notices report the number of toggled entries. File availability uses the existing visible-file predicate; folder availability uses a new visible-folder predicate derived from visible-entry/file counts. No new shortcut or filesystem mutation introduced.

Verification:

- Full local suite: **2413 passed, 0 failed, 3 ignored**. New panel test covers per-kind inversion, reversibility, filtered views and empty-view no-ops. Workspace test dispatches both commands and verifies other-pane/focus retention and per-kind availability under file-only/folder-only filters.
- The panel regression was then strengthened to explicitly seed selected entries hidden by the filter. Both **2 targeted inversion tests passed** again. Production behavior unchanged by this test refinement.
- All-target Clippy with visual-qa and warnings denied passed. Final formatting and scoped whitespace checks passed.

No failed checks. No native palette interaction, remote CI, commit or push performed.
