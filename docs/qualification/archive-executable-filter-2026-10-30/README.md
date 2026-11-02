# Archive executable regular-file filter — 2026-10-30

Independent adaptation of [fd executable-file filtering](https://github.com/sharkdp/fd/blob/master/doc/fd.1) (`--type executable`), cohort #77. Types now offers Executable files only, using existing ArchiveMember metadata rather than probing the host or invoking archive contents.

The additional predicate requires is_regular, excludes directory/symlink classifications even if flags overlap, and requires is_executable == Some(true). Known false and missing status both fail. It intersects the existing type choices, so Regular files must also be enabled; other enabled type choices do not bypass this constraint. This is declared archive metadata, not an effective host permission/access test or proof that a payload can safely run.

The flag belongs to Kinds identity, so toggling rebuilds visible rows and saved snapshots distinguish identical queries with different executable constraints. Saved labels retain both base type choices and the additional executable requirement. Recall restores it; Show all types and Reset clear it. Folder navigation remains available through synthetic rows, and hidden selections keep their existing copy/extraction membership. No new read, worker or execution is added.

Two regressions cover true/false/unknown status, directory/link/other exclusion (including overlapping flags), base-type intersection, cache toggling, synthetic navigation, hidden selection and saved/Reset restoration. Native Types interaction, recall, focus and menu/layout remain pending: CUA is not exposed in this continuation, no native launch was attempted, and earlier archive captures do not qualify the changed menu or labels.

Validation: locked visual-QA suite passed 2821 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source/binary hashes are recorded alongside the logs. No commit or push performed.
