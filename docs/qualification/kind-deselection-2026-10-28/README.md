# Deselect visible files/folders independently — 2026-10-28

Independent adaptation of [Double Commander attribute-filtered Unselect All](https://doublecmd.github.io/doc/en/configuration.html). No upstream code copied. This exposes fixed file/folder targets rather than a general attribute-mask language.

The palette offers Deselect visible files and Deselect visible folders. They remove only selected entries of that kind from the current filtered view, retaining the other kind, filtered-out picks and picks remembered in other folders. No cursor fallback or parent-row selection occurs. Selection revisions flow through the existing removal methods. Full command-context selected-file counts gate each command on actual selected visible entries; unavailable entries explain the missing kind. Dispatch reports the removed count using existing notices.

Validation:

- Full local suite: **2440 passed, 0 failed, 3 ignored**. Panel regression covers mixed visible/filtered file-folder picks, selections remembered elsewhere, per-kind removal counts, repeated no-op application, unchanged cursor/filter and retention of unrelated selections.
- Both-pane command regression checks selected-visible-kind availability before/after each subtraction, a parent cursor without fallback, hidden-by-filter retention, focus and other-pane selection independence.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native palette interaction/capture was run for these entries. No commit or push.
