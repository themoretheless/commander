# Find parent folders — 2026-10-28

Independent adaptation of Yazi `copy dirpath`, cohort repository #27 ([primary documentation](https://yazi-rs.github.io/docs/configuration/keymap/#copy)). No upstream code copied.

Find Copy as → Parent folders supports all visible results and the selected subset. It copies distinct absolute containing folders in first-visible order; directory hits use their parent and archive members use the physical archive’s containing directory. Roots without a parent are omitted; empty scopes do not copy. Relative legacy targets resolve lexically without filesystem reads. The copied count reports folders rather than hits. The format remains runtime-only.

Shared clipboard deduplication now uses native PathBuf keys before lossy display/control escaping. Distinct names that render identically remain distinct entries; output remains escaped plain text. Regressions cover hidden omission, selection, order, duplicate native/archive parents, directory hits, empty selection and root-only results, plus a literal-backslash/newline rendering collision.

Validation: full locked visual-qa suite **2590 passed, 0 failed, 3 ignored**. All-target Clippy with warnings denied passed. Native scenario extends prior stages to choose Parent folders, copy selected and all-visible results, require the same single fixture folder with twelve recording clipboard writes, and retain query focus, selection and five tabs. Native capture results are recorded alongside this report.

Fresh CUA-launched native capture passed **16/16 checks**, with exactly twelve recording clipboard writes and other native effects zero. The selected and all-visible copies both produced the fixture’s single containing folder. Final phase 126 retains query focus, README selection, three results and five tabs. Frame inspected: Parent folders and both copy buttons fit without clipping. Real system clipboard untouched; the QA application captured and exited before CUA attachment. Locked build, formatting and whitespace checks passed. No commit or push.
