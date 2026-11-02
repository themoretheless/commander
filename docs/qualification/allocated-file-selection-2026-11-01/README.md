# Select the ten largest files by allocated size

Continues the independent [Dust biggest-file/disk-usage selection adaptation](https://github.com/bootandy/dust/blob/master/README.md), cohort #82: its `-n` and `-F` overview informs an additive file-manager selection command, not a copied implementation.

**Select 10 largest allocated files** ranks visible regular files with observed allocation and adds the top ten paths. Unknowns, folders and symlinks do not become candidates. Known zero is valid, and fewer than ten candidates are all selected. Existing picks, including hidden ones, remain; repeated application is idempotent. Stable `(bytes, path)` ties match the existing logical-size selector independently of view order. The bounded heap is shared by both selectors; existing logical behavior is unchanged.

The palette's shared availability predicate uses allocation facts gathered during its existing filtered-view pass, without a new filesystem probe. It disables the command when all visible candidates lack allocation facts. This is selection only, not a reclaimable-space estimate; hard links and shared extents remain individual paths and can share blocks.

## Validation

- Three new regressions cover allocated/logical contrast, zero/unknown/folder exclusion, hidden and existing picks, n=0/usize::MAX, idempotence, cursor preservation, top-ten limit, stable ties in both display directions, real Unix sparse/dense/link observations and command availability for a link-only view.
- Full suite **3119 passed / 3 ignored**, six commands in `checks.json` zero. External RHash and b3sum coverage remains enabled and recorded in `validators.json`.
- Native `allocated_selection` invokes actual allocation selection, JSON export, legacy logical selection and another JSON export. The fixture starts with one hidden selected note; twelve increasingly allocated files compete with a large sparse file, an empty file and a link. Allocation selection exports ten files and preserves the hidden note. Legacy logical selection then adds the sparse file and its link, demonstrating the distinction without replacing existing picks.
- Native `listing_json` reruns the previous two-export/projection workflow on the same executable. Both scenarios **14/14**, two recording clipboard calls each and all other native-port effects zero. Final screenshots are inspected and retained durably under native-captures.
- `verify.py` parses both selection reports, independently ranks the fixture's observed allocation values, checks preservation/projection counts, command outcomes, final executable hashes and native manifests.

No recursive selection, hard-link collapsing or new allocation measurement is introduced. The inherited listing facts can be refreshed using the normal pane refresh.
