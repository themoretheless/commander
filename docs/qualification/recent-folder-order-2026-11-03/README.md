# Recent-folder ordering controls

Continues the [zoxide-inspired ranked-directory navigation](https://github.com/ajeetdsouza/zoxide/wiki/Algorithm) from cohort entry 78. Commander already had its own frequency/recency score, chronological alternative and persisted RecentOrder enum; this change exposes the otherwise unreachable alternative. It does not copy zoxide source or claim zoxide offers this UI toggle or the same score formula.

Cmd+P Jump mode offers Frequent + recent and Recent first. Only recent destinations change order; pinned favorite rows stay first, and the query and visit counts/timestamps remain intact. The existing chronological projection preserves the most-recent-first path trail; the default frecency projection continues to combine bounded frequency/recency with fuzzy score.

Changing modes resets selection and restores input focus, retaining the open switcher. The form suppresses same-frame Enter/navigation and any stale forget request while the projection changes. The existing session service saves the chosen order immediately; save failure reports that the choice is retained only in this session. Removal and order changes share the same persistence helper.

Regressions exercise real egui pointer selection and focused Enter ownership; contrasting newest-vs-frequent projections for empty and filtered queries without mutating visits; and both ordering modes through full session JSON round-trip with paths/ranks preserved. Existing missing-field compatibility still defaults to Frecency. Native toggle/persist/reopen UI and save-failure rendering remain separate scope.

Final validation: all seven gates exited 0; tests 3367 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. verify.py checks the three named regressions, terminal results and matching source hashes.
