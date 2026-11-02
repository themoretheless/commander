# Optional archive search — 2026-10-28

Independent adaptation of Double Commander's [Search in archives option](https://doublecmd.github.io/doc/en/findfiles.html), from the researched 99-repository cohort. No upstream code copied.

Find accepts `archives:off` and `archives:on` (aliases `false`/`true`, case-insensitive). Without this option existing archive expansion remains enabled. The control determines whether archive members are inspected; it does not remove archive files from ordinary name/metadata search results. Conflicting controls fail validation, including directly constructed/deserialized predicates. Expressions, chips and saved JSON preserve the setting.

The search compiler passes the scope to live/indexed providers and to the central archive reader guard. Disabled runs do not expand or open archive contents, and their provider labels state `archives off`. Existing trust, depth, exclusion, cancellation and archive budgets still apply when expansion is enabled. Index construction is unchanged; the control governs the search run, not prior indexing activity. This is a saved query control, not a new checkbox.

Regression coverage includes all text modes, aliases, serialization and conflicting options. Real live/indexed fixtures compare off, explicit on and default-on behavior with an ordinary text file, valid ZIP member and invalid ZIP. Disabled runs return only the ordinary content hit without archive errors; enabled/default runs include the member and report the broken archive. Separate disabled name queries still return both ZIP files, proving that archive filenames remain searchable. Provider label scope is checked. No native UI interaction or capture; no commit or push.

Validation: **2483 passed, 0 failed, 3 ignored** in the full locked suite. Both focused regressions passed. All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.
