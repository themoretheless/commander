# Sort the file pane by allocated size

Continues the independent [Dust disk-usage/apparent-size adaptation](https://github.com/bootandy/dust/blob/master/README.md), cohort #82.

**Sort by allocated size** sorts captured regular-file block allocations in the active pane. Repeating the command or using its **Allocated** header reverses known values. Unknown values remain after known ones in either direction; existing folder pinning and folders-by-name preferences still take precedence. Equal allocation uses ascending configured name/path ties. Directory and symbolic-link target allocations do not enter this metric.

The Size column becomes Allocated and displays allocation values. Accessibility retains the logical size and explicitly adds allocated size. **Sort by size** returns to logical size/order. No filesystem read is added to sorting/rendering, and sorting preserves the existing cursor, selections and per-directory/tab view model. The new enum value round-trips through saved JSON view settings. Existing totals, transfers, search predicates and folder logical-size measurement do not change meaning.

## Validation

- Two new regressions cover known zero, differing apparent/allocated order, stable natural-name ties, unknown-last ascending/descending, folder policy, actual command dispatch, cursor/selection preservation and view serialization. The existing symlink allocation test now also checks its sort metric is unknown.
- Full suite **3110 passed / 3 ignored**. All six commands in `checks.json` exit zero.
- Native `allocated_sort` invokes actual palette commands: allocation ascending → allocation descending → logical size ascending → allocation ascending. Four retained snapshots expose original logical values and the sort metric; a sparse file reverses relative order with an allocated dense file. The symlink remains unknown and after known files. Cursor stays on sparse.bin.
- Existing native `allocated_info` checks the previous sparse-file Info feature on the same executable. Both **14/14**, recorded native-port effects zero. The final allocation-column frame is visually inspected.
- `verify.py` independently validates command results, binary hashes, manifests, all four sort snapshots and cursor/metric preservation.

This sorts observed single-file allocation; it does not calculate recursive directory allocation, exclusive ownership, reclaimed space or Windows allocation. View changes retain listing-time facts until refresh.

The first strict Clippy pass found a redundant binding in the new test. It was removed; earlier logs remain under `before-test-lint-fix/`. Final captures are reused only when their executable hashes equal the final build.

On 2026-11-01, the post-pause verifier found the original `/tmp` captures had expired before they were copied here. Both native scenarios were recaptured with the same verified executable into `native-captures/`, then `verify.py` passed and the new allocation-column screenshot was inspected. `prepare_capture.py` now prepares durable capture destinations; launch the prepared CommanderFolderQA app through CUA. Earlier launch records are preserved inside each new capture directory.
