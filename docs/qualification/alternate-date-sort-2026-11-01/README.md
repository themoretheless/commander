# Accessed and created sorting

Independent adaptation of [eza alternate sort/time fields](https://github.com/eza-community/eza/blob/main/man/eza.1.md), cohort #79. No implementation copied.

The palette commands **Sort by accessed** and **Sort by created** sort the retained listing, flip direction when repeated, and persist in ViewConfig. The date header/value/hover follows Accessed or Created; Sort by modified restores legacy Modified. Missing dates remain last in both new directions. Folder policies still take priority; equal known/unknown values use configured ascending name order and stable path ties. Existing Modified unknown ordering remains unchanged.

FileEntry captures optional dates and absolute display strings with the existing metadata calls. Directory listing and one-object observation use lexical metadata for these new dates: links have their own times, never the target's. No sort/Info/JSON-time metadata calls are added. Get Info displays both dates and marks link-own observations. JSON v1 adds nullable accessed_utc/created_utc with the existing UTC subsecond formatter. Synthetic rows retain unknown values. Ordinary date-column rendering reuses its existing relative label; it does not compute a second date label unless an alternate metric is active. Accessibility preserves Modified and appends the active metric.

## Validation

Final suite: **3133 passed / 3 ignored**; all six checks exit zero, external checksum oracles enabled. Visual-QA Clippy passes with -D warnings. Ordinary cargo check retains the unrelated unused `response` warning in src/diff_find.rs:136. All three native captures passed **14/14**, recording clipboard counts 0/2/0 and all other native effects zero. Screenshots inspected; the independent verifier passes for all six raw-derived orders, selection-context JSON dates and current executable/tool hashes.

Five regressions cover subsecond/pre-epoch ordering, independent metrics, stable natural-name ties, unknown-last directions, folder policies and serialized view configuration; actual commands/direction/notices/cursor/selection; UTC JSON precision and unknowns after source removal; native metadata snapshots and Info after removal; and lexical link dates versus deliberately different target access time. Existing folder-name policy tests cover both new columns.

Native time_sort uses three ordinary files with deliberately opposite access/modified order and independently observed creation times. Six actual palette commands cover Accessed asc/desc, Created asc/desc, Modified, and Accessed again, retaining cursor and visible/hidden selections. Each stage records filtered rows, settings and a selection-context JSON report; verification derives expected order from independently captured raw metadata, not Commander sorting code. listing_json and symlink_info are neighboring export/Info regressions. All captures are bound to the current executable hash.

See checks.json, validators.json, tests.log, verification.log and native-captures for final results. Access time can be approximate, deferred or disabled by filesystem policy. Creation time is optional and is never replaced by ctime or modification time. Dates are snapshots until refresh; viewing/opening file contents elsewhere can change future access observations. Native cases use the host filesystem; they do not qualify creation-time support on every platform.

On this macOS/APFS host, setting a fixture modification date earlier than its birth time also moved the reported creation time back. Native expectations use the actual separately captured creation times rather than assuming fixture creation order. The sort commands themselves never set file times.
