# Code audit: top 50 things done badly or wrong (round 2)

A critical review of the commander codebase, refreshed. Round 1 ran 12
file-scoped reviewers (109 raw findings -> top 50). Round 2 re-ran 12
**dimension-scoped** reviewers (panics, concurrency, egui, perf, FFI,
data-integrity, error-handling, API-invariants, resource-leaks, fs-edge,
lifecycle, testing) told to find what round 1 missed, then merged the still-open
round-1 items with the new ones into the fresh top 50 below. **Items already
fixed are removed from the list** (see "Resolved since round 1"). This complements
[architecture.md](architecture.md) (structural debt) and feeds
[recommendation.md](recommendation.md) (the plan).

## How to read this

Each row carries a reviewer `severity`/`confidence` and a `Src`:

- `new` = surfaced in round 2 (a defect round 1 did not catch).
- `carried` = still-open round-1 finding (re-confirmed, not yet fixed).

I **manually re-verified the highest-severity NEW items against the source**
(transfer.rs swap/symlink, image_cache overflow, panel cursor/index, the
partial-undo path, the duplicates index); the corrections below matter because
the raw ranking over-weights theoretical concurrency and over-states a couple of
impacts. Medium/low-confidence rows are reviewer-reported leads to confirm, not
settled facts.

## Resolved since round 1 (removed from the list)

Fixed in commit `177e67c` (and excluded from the round-2 ranking):

| Round-1 # | Item | Resolution |
| --- | --- | --- |
| 2 | Image cache not flushed on directory change | `image_cache` preload now clears `entries`/`total_bytes` and resets `current_dir` when the directory changes. |
| 7 | `write_atomic` bool ignored in all 4 save fns | `session`/`bookmarks`/`smart_folder`/`cmdtemplate` `save()` now return the success bool; the Find, Saved-search and Run-command dialogs toast on failure. |
| 20 | `Result` swallowed in the undo/redo apply path | `perform_undo`/`perform_redo`/`execute_action` now return `Result<(),String>`; the app surfaces an "Undo failed" toast instead of dropping the error. |
| 11 | `find_dialog` Enter "needs `lost_focus()`" | **Dropped as a false positive** (never a bug): `resp.lost_focus() && key_pressed(Enter)` is the standard egui submit idiom. |

Note: round-1 #3 (the `rename_order` internal rollback being non-atomic and
dropping its own `Err`) was **not** closed by the undo-layer fix above; it is
still open and now appears as round-2 rank 8.

## Verification corrections (re-checked by hand, round 2)

| Rank | Item | Reviewer said | Verified verdict |
| --- | --- | --- | --- |
| 1 | `swap_into_place` `exists()` vs rename | high / "silent clobber, overwrites the link target" | **Real inconsistency, framing overstated.** The genuine defect is `dest.exists()` (510) diverging from the `path_is_taken()` used at 264; `exists()` follows symlinks, so a broken symlink takes the no-backup branch. But `rename()` atomically *replaces* a broken symlink (it does not "overwrite the link target"), so there is no data loss there. The concurrent-writer TOCTOU window is real but narrow. Fix for consistency + no-clobber rename; effective severity medium. |
| 2 | image/video pixel-buffer integer overflow | high / heap corruption | **Real, gated.** `w*4` and `h*bytes_per_row` are unchecked `usize` mults (image_cache.rs:341-342, 516-517); only zero is checked. Triggering needs CoreGraphics to actually *decode* a pathologically large image first, which it usually refuses. `checked_mul` + a dimension cap is cheap defensive hardening. Keep high as defense-in-depth. |
| 5/6 | cursor / `filtered_entries[i]` not bounds-checked | high | **Confirmed mechanism.** `selected_or_cursor()` returns `vec![]` when `filtered_get(cursor-1)` is `None` (panel.rs:1524-1527), and `filtered_entries()` indexes `self.entries[i]` directly (1463). Both are reachable if a filter tightens or `entries` shrink without a generation bump. Real defensive-hardening items. |
| 11 | undo of Move skips deleted sources | high | **Confirmed.** `start_move_silent` `filter_map`s away sources whose `metadata()` fails (workspace.rs:1060-1066); the `is_empty()` guard only catches a fully-empty set, so a partial undo restores N-1 of N silently. Real data-integrity inconsistency. |
| 18 | CallbackCtx stack pointer to `copyfile` | high / use-after-free | **Overstated (carried).** `copyfile()` is synchronous and blocks the frame; the callback only fires while `ctx` is alive. Fragile pattern, not a live UAF. Box the ctx defensively. |
| 17 | nested `fs_pool().install()` | high / deadlock | **Real anti-pattern, "deadlock" unproven (carried).** The `install()` is redundant (the closure already runs on the pool); realistic starvation needs the pool fully saturated by these very tasks. Low-medium: remove the nesting. |

## Top 50 (synthesis rank; `*` = severity adjusted by manual verification)

| # | Sev | Conf | Cat | Src | Issue | Location |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | high* | high | bug | new | TOCTOU in `swap_into_place`: `exists()` not atomic with rename and diverges from `path_is_taken()` (symlink framing overstated) | transfer.rs:509-514 |
| 2 | high | high | bug | new | Unchecked integer overflow in image/video thumbnail pixel-buffer alloc (gated on CG decoding a huge image) | image_cache.rs:341-342,516-517 |
| 3 | high | high | concurrency | carried | `lock().unwrap()` panics on poisoned transfer state (many call sites) | transfer.rs:219..666 |
| 4 | high | high | error-handling | carried | Duplicates-dialog commit indexes `s.keep[gi]` without bounds check | app/duplicates_dialog.rs:213-217 |
| 5 | high | high | bug | new | Cursor not re-clamped when filter/facets change without a reload | panel.rs:1163-1170,1441-1450,1518-1530 |
| 6 | high | high | bug | new | `filtered_entries()` indexes `self.entries[i]` from a possibly-stale cache | panel.rs:1460-1464 |
| 7 | high | high | bug | carried | `gather_into_folder` leaves an orphan folder/files when a move fails | workspace.rs:1174-1194 |
| 8 | high | high | bug | carried | Rename rollback non-atomic; failed rollback `Err` dropped | rename_order.rs:56-64 |
| 9 | high | high | bug | carried | Reserved-set casing mismatch yields colliding temp names | rename_order.rs:188-202 |
| 10 | high | high | error-handling | carried | `batch_rename` commit `unwrap` can panic | app/batch_rename_dialog.rs:243 |
| 11 | high | high | bug | new | Undo of Move silently skips deleted sources -> partial undo | workspace.rs:1062-1068 |
| 12 | high | high | error-handling | new | `copy_symlink` ignores `remove_file` failure, masking perm/lock errors | transfer.rs:637 |
| 13 | high | high | error-handling | new | Panicking `unwrap` on ObjC class lookup at menu `show()` time | native_menu.rs:460,50 |
| 14 | high | med | testing-gap | carried | No on-disk undo round-trip integration test | undo.rs / workspace.rs |
| 15 | high | high | perf | carried | `NameContains` re-lowercases the query per entry | query.rs:14-16,30 |
| 16 | high | high | perf | new | `select_by_relation` double-clones all filtered entries | workspace.rs:379-384 |
| 17 | high* | med | concurrency | new | Nested `fs_pool().install()` inside `spawn()` (redundant; deadlock unproven) | panel.rs:1027-1048,1054-1088 |
| 18 | low* | med | bug | carried | CallbackCtx stack pointer to `copyfile` callback (fragile, not a live UAF) | native_copy.rs:211-226,287-303 |
| 19 | med | high | error-handling | new | Spawned dir-size scan errors silently discarded (persistence half already fixed) | panel.rs:1027-1089 |
| 20 | med | high | bug | carried | Watcher callback can fire for the old dir after rapid navigation | panel.rs:890-936,1163-1171 |
| 21 | med | high | concurrency | new | `compute_dir_sizes` clears maps before in-flight tasks finish writing | panel.rs:943-950 |
| 22 | med | high | bug | new | `notify` `Arc<dyn Fn>` may be invoked after owning panel/workspace dropped | transfer.rs:207-246, panel.rs:902-927 |
| 23 | med | high | concurrency | carried | Transfer worker `JoinHandle` discarded; no join before context drop | transfer.rs:207-212 |
| 24 | med | med | concurrency | new | `lock().unwrap()` inside the `copyfile` C callback can panic on poison | native_copy.rs:125..322 |
| 25 | med | high | error-handling | new | `swap_into_place` restore failure orphans original at hidden backup path | transfer.rs:509-532 |
| 26 | med | high | ui-correctness | new | Find-dialog results `ScrollArea` lacks a unique `id_salt` | app/find_dialog.rs:197-228 |
| 27 | med | high | ui-correctness | new | Batch-rename preview `ScrollArea` lacks a unique `id_salt` | app/batch_rename_dialog.rs:154-175 |
| 28 | med | high | ui-correctness | new | Run-command templates `ScrollArea` lacks a unique `id_salt` | app/run_command_dialog.rs:107 |
| 29 | med | high | concurrency | carried | `lock().unwrap()` poisoning panics in image_cache/confirm_dialog | image_cache / confirm_dialog |
| 30 | med | high | concurrency | carried | Non-atomic mark-pending then spawn in image preload | image_cache.rs:118-141 |
| 31 | med | med | bug | new | image_cache `pending` entries never pruned on directory change | image_cache.rs:97-101,131-144 |
| 32 | med | med | bug | new | `page_rows` is 0 until first render, breaking PageUp/Down pre-render | panel.rs:729,773 |
| 33 | med | high | error-handling | new | `selected_or_cursor()` silently returns empty on out-of-bounds cursor | panel.rs:1518-1531 |
| 34 | med | high | perf | carried | `FontId` cloned per char per row per frame in highlight rendering | app/file_list.rs:687-698 |
| 35 | med | high | perf | carried | O(n^2) `Shelf::add`/`add_all` via linear `contains` | shelf.rs:17-26 |
| 36 | med | high | perf | new | `navigate_to_file` does an allocating O(n) scan of `filtered_entries` per keystroke | workspace.rs:1484 |
| 37 | med | med | perf | new | `filtered_entries()` allocates a fresh `Vec` on every call (~18 callers) | panel.rs:1460-1464 |
| 38 | med | high | perf | new | `walk_log` HashMap grows unbounded with no pruning | panel.rs:60-64 |
| 39 | med | high | perf | new | `dir_size_cache` grows unbounded between `flush_cache()` calls | panel.rs:28-32,1077-1082 |
| 40 | med | med | ui-correctness | carried | Compare cache not cleared on toggle-off (stale generations) | app/update.rs:669-681 |
| 41 | low | med | ui-correctness | carried | Toast `Id` from reversed list index, unstable under coalescing | app/update.rs:924-932 |
| 42 | med | high | perf | carried | `speed_samples` uses O(n) `Vec::remove(0)` on the hot path | transfer.rs:139-141 |
| 43 | low | high | perf | carried | `group_duplicates` clones every `FileKey` into buckets | dedup.rs:47 |
| 44 | med | med | bug | new | `is_valid_name` accepts control chars/newlines in filenames | rename.rs:96-98 |
| 45 | med | high | error-handling | carried | `nsstring` swallows `CString::new` errors -> empty labels | native_menu.rs:22-24 |
| 46 | med | med | error-handling | new | `cleanup_path` partial `remove_dir_all` failure dropped, orphans source tree | transfer.rs:471-480 |
| 47 | med | high | error-handling | carried | Empty find term silently disables find/replace | rename.rs:80-84 |
| 48 | med | med | perf | carried | `opqueue` `get_state`/`set_state` are O(n) by `JobId` | opqueue.rs:137-139,278-282 |
| 49 | med | med | bug | carried | Negative float cast to `usize` in file_list size math | app/file_list.rs:189 |
| 50 | low | med | testing-gap | carried | Empty-input CSV/Markdown export emits header-only output, untested | listing_export.rs:126-127 |

## Themes (clusters worth fixing together)

1. **Reachable panics.** `lock().unwrap()` poisoning (3, 24, 29), the ObjC
   `unwrap`s (13), the `batch_rename` unwrap (10), and the unchecked indices
   (4, 6) can all crash the UI. Each should fail soft (`unwrap_or_else(|e|
   e.into_inner())`, `.get()`, `Result`) or be made provably unreachable. This is
   recommendation **D6**.
2. **Panel filter/cursor invariants.** The cursor-not-clamped (5), the silent
   empty `selected_or_cursor` (33), and the stale-index `filtered_entries` (6)
   all stem from `PanelState` exposing `cursor`/`entries`/`filter_cache` as raw
   public fields that can drift out of sync. An `ensure_cursor_valid()` helper
   plus a borrowed iterator accessor closes most of them, and they are the
   strongest argument for the `ViewState`/encapsulation work in
   [architecture.md](architecture.md).
3. **The dir-size index is unsafe shared state.** The clear/spawn race (21), the
   nested pool (17), the stale watcher callback (20), the unbounded `walk_log`
   (38) and `dir_size_cache` (39) all live in `panel.rs`'s raw
   `Arc<Mutex<HashMap>>` background plumbing. This is exactly the `DirIndex` /
   `BackgroundScan` extraction the architecture doc recommends (recommendation
   **D5**, paired with the extraction).
4. **Destructive-op partial failure.** Non-atomic rename rollback (8), the
   orphan folder on a failed gather (7), the partial undo of Move (11), the
   orphaned backup on a failed swap restore (25), and the dropped `cleanup_path`
   error (46) all leave the filesystem in a half-done state with no clear report.
   These deserve one careful pass with on-disk integration tests (14).
5. **Per-frame / per-keystroke allocation.** Per-character `FontId` clone (34),
   the shelf O(n^2) (35), the per-entry re-lowercasing (15), the double-clone in
   `select_by_relation` (16), the per-keystroke linear scan (36), and the
   fresh-`Vec`-per-call `filtered_entries` (37) are cheap, isolated wins
   (recommendation **D4**).
6. **egui widget-Id hygiene.** Three dialog `ScrollArea`s lack an `id_salt`
   (26, 27, 28) and the toast Id is index-derived (41); add stable salts to avoid
   cross-widget scroll/focus bleed.
