# Select/deselect files sharing the cursor stem — 2026-10-28

Independent adaptation of [Double Commander cm_MarkCurrentName / cm_UnmarkCurrentName](https://doublecmd.github.io/doc/en/cmds.html). No upstream source or dependency copied.

The command palette now offers Select files with cursor's name (ignore extension) and its Deselect counterpart. Both compare the cursor file’s name without the last extension using existing Unicode-lowercase name keys. For example, report.pdf groups with report.docx, Report.TXT and extensionless report. archive.tar.gz groups with archive.tar.xz, not archive.zip. Only visible files change. Folders, unrelated picks, filtered-out selections and the opposite pane remain. Parent/folder cursor rows disable both actions through the existing cursor-file capability predicates. No new hotkey or filesystem mutation added.

Verification:

- Full local suite before the later test refinements/addition: **2408 passed, 0 failed, 3 ignored**.
- Final scoped **3 tests passed**: add/deselect/idempotence, unrelated and filtered-out picks, parent/folder no-ops, last-extension grouping; real Workspace command dispatch/capability gating and opposite-pane/focus preservation. The panel fixture was refined to give the folder a distinct path; the workspace integration test was added after the broad run.
- Final all-target Clippy with visual-qa and warnings denied, formatting and scoped whitespace checks passed.
- Rejected test-only compilation retained: the new workspace test initially attempted to call PanelState’s private filtered iterator; corrected to the public focus_child navigation operation. Production unchanged by the repair.

No final full-suite rerun, native palette interaction, remote CI, commit or push claimed.
