# Code audit: top 50 things done badly or wrong

A critical review of the commander codebase: 12 reviewers each scoped to a
cluster of files produced 109 raw findings; a synthesis pass deduplicated and
ranked them into the top 50 below. This complements [architecture.md](architecture.md)
(structural debt) and feeds [recommendation.md](recommendation.md) (the plan).

## How to read this

Each finding was produced by an automated reviewer and carries the reviewer's
own `severity` and `confidence`. I then **manually re-verified ~10 of the
highest-severity items against the source**; the corrections below matter,
because the raw ranking over-weights theoretical concurrency issues and contains
one false positive. The remaining ~40 items are reviewer-reported and were not
each individually re-verified, so treat medium/low-confidence rows as leads to
confirm, not settled facts.

## Verification corrections (re-checked by hand)

| Rank | Item | Reviewer said | Verified verdict |
| --- | --- | --- | --- |
| 1 | `native_copy` CallbackCtx on the stack | high / use-after-free | **Overstated.** `copyfile()` (native_copy.rs:233) is synchronous and blocks the frame; the callback only fires during that call while `ctx` is alive. Not a live bug, a fragile pattern. Downgrade to low (defensive: box the ctx). |
| 11 | `find_dialog` Enter needs `lost_focus()` | high / Enter does nothing while focused | **False positive.** `resp.lost_focus() && key_pressed(Enter)` is the standard egui idiom: pressing Enter in a singleline surrenders focus the same frame, so submit works. Drop. |
| 5 | `compute_dir_sizes` clears then spawns | high / "permanent data loss" | **Real race, severity overstated.** `dir_sizes`/`dir_counts` are a derived cache, recomputed on the next refresh: the symptom is size flicker and wasted work, not data loss. Medium. |
| 6 | nested `fs_pool().install()` | high / deadlock | **Real anti-pattern, "deadlock" overstated.** The `install()` is redundant (the closure already runs on the pool); with 10 threads and 2 spawned tasks there is no realistic starvation. Low-medium: remove the nesting. |
| 8 | unbounded loops in `first_available`/`free_name_against` | high / hangs the app | **Theoretical only.** The loop ends as soon as a free name is found; hanging needs ~2^64 colliding names, which no disk can hold. Low: add a cap as defensive hardening. |
| 2 | image cache not cleared on dir change | high | **Confirmed.** Docstring (image_cache.rs:90) promises a flush; code only updates `current_dir`. Real (stale previews). Keep, but it is a UX bug, not data loss: medium. |
| 7 | `write_atomic` return ignored in 4 save fns | high | **Confirmed.** All four ignore the bool; saves can fail silently. Session save is "best-effort" by design, but bookmarks/smart-folders/templates are explicit user actions and should surface failure. Medium. |
| 12 | `size_max` O(n) per frame | high | **Confirmed**, but gated on the size-bars feature being on. Real per-frame cost for big dirs; medium. |
| 26 | size-filter chips share one field | medium | **Confirmed** (render.rs: both `>1MB` and `>100MB` write `min_size`). Real UX bug. |
| 3 / 20 | error swallowed via `let _ =` in rename rollback / undo apply | high / medium | **Confirmed** (rename_order rollback and `apply_rename_order` in undo both drop their `Result`). Real. |

Net: the genuinely high-priority, confirmed problems are the **error-swallowing
on save/undo** (7, 3, 20), the **dir-size index concurrency** (5, 40), the
**image-cache eviction** (2), and the cheap **per-frame perf** wins (12, 29, 30).
Items 1, 8, 11 are the weakest of the top tier.

## Top 50 (synthesis rank; severity is the reviewer's unless corrected above)

| # | Sev | Conf | Category | Issue | Location |
| --- | --- | --- | --- | --- | --- |
| 1 | low\* | high | concurrency | CallbackCtx stack pointer handed to `copyfile` (fragile, not a live UAF: copyfile is synchronous) | native_copy.rs:211-226 |
| 2 | med\* | high | bug | Image cache not flushed on directory change despite docstring | image_cache.rs:91-97 |
| 3 | high | high | bug | Rename rollback is non-atomic; failed rollback `Err` dropped, files stranded at `.cmdr-rename-*` | rename_order.rs:56-64 |
| 4 | high | high | bug | `gather_into_folder` leaves an orphan empty folder if the move fails | workspace.rs:1174-1194 |
| 5 | med\* | high | concurrency | `compute_dir_sizes` clears maps before spawned tasks finish (flicker) | panel.rs:945-950 |
| 6 | low\* | high | concurrency | Redundant nested `fs_pool().install()` inside spawned tasks | panel.rs:1027-1070 |
| 7 | med\* | high | error-handling | `write_atomic` bool ignored in all 4 save fns; silent save failure | session.rs:78, bookmarks.rs:180, smart_folder.rs:54, cmdtemplate.rs:244 |
| 8 | low\* | high | error-handling | Unbounded name-allocator loops (no practical trigger) | fs_util.rs:106-118, 145-170 |
| 9 | high | high | bug | Temp-name reserved set mixes lowercased keys with exact-case temps | rename_order.rs:188-202 |
| 10 | high | high | error-handling | `batch_rename.as_ref().unwrap()` inside commit can panic | app/batch_rename_dialog.rs:243 |
| 11 | drop\* | high | bug | (False positive) Enter handling in find dialog is the egui idiom | app/find_dialog.rs:66 |
| 12 | med\* | high | perf | O(n) `size_max` scan over all filtered entries every frame | app/file_list.rs:175-181 |
| 13 | high | high | perf | `NameContains` re-lowercases the query for every entry | query.rs:14-16,30,69 |
| 14 | med | high | error-handling | `default_keep()` returns index 0 for an empty group | dedup.rs:74,81 |
| 15 | med | med | bug | Unchecked `keep[gi]` index in duplicates dialog | app/duplicates_dialog.rs:213 |
| 16 | high | high | testing-gap | No integration test that a real Move/BatchRename undoes the on-disk state | undo.rs / workspace.rs |
| 17 | med | high | bug | TOCTOU in `swap_into_place`: `exists()` not atomic with the rename | transfer.rs:509-512 |
| 18 | med | med | bug | Conflict detection races with fs changes between dialog open and confirm | workspace.rs:741-754 |
| 19 | med | med | bug | Recorded undo placements may not match on-disk state if files change | workspace.rs:933 |
| 20 | med | high | error-handling | `apply_rename_order` `Result` dropped via `let _` in undo/redo | workspace.rs:1031 |
| 21 | med | high | perf | `Vec::remove(0)` on the speed-sampling hot path is O(n); window off-by-one | transfer.rs:139-141 |
| 22 | med | high | error-handling | `lock().unwrap()` panics on poisoned locks across modules | image_cache.rs, app/confirm_dialog.rs:64, transfer.rs:334-348 |
| 23 | med | high | concurrency | Transfer worker `JoinHandle` discarded; no wait-on-completion before teardown | transfer.rs:207-212 |
| 24 | med | high | bug | Compare cache cleared on toggling compare off, forcing rebuild | app/update.rs:669-681,815 |
| 25 | med | high | ui-correctness | Toast `egui::Id` derived from list index; unstable as toasts expire | app/update.rs:924-932 |
| 26 | med | high | ui-correctness | Size-filter chips (`>1MB`,`>100MB`) share one field, overwrite each other | app/render.rs:53-66 |
| 27 | med | high | error-handling | Clipboard copy silently dropped with no toast when selection empty | app/update.rs:156-179 |
| 28 | med | high | bug | Shelf self-copy check is lexical, misses symlinked destinations | shelf.rs:68 |
| 29 | med | high | perf | `FontId` cloned per character per visible row per frame | app/file_list.rs:687-698 |
| 30 | med | high | perf | O(n^2) dedup in `Shelf::add`/`add_all` via `Vec::contains` | shelf.rs:17-26 |
| 31 | med | high | perf | `opqueue` `get()`/`set_state()` O(n) linear scan per transition | opqueue.rs:137-139,278-282 |
| 32 | med | high | perf | `group_duplicates` clones every `FileKey` while bucketing | dedup.rs:47 |
| 33 | med | med | error-handling | `set_state()` silently no-ops on unknown `JobId` | opqueue.rs:278-282 |
| 34 | med | med | testing-gap | `reorder()` not verified against actual `dequeue_next` order | opqueue.rs:456-487 |
| 35 | med | med | error-handling | Run-command line expanded 3x, no error surfaced on failure | app/run_command_dialog.rs:83,139,172 |
| 36 | med | high | api-design | Empty `find` silently disables find/replace; contract undocumented | rename.rs:80-84 |
| 37 | med | med | perf | Unvalidated numbering pad/overflow can allocate huge strings | rename.rs:90-91 |
| 38 | med | low | error-handling | `relative_date` uses non-portable `%-d` strftime extension | reldate.rs:53,55 |
| 39 | low | high | testing-gap | `pause/resume/reorder/promote` have no integration coverage with the drain | opqueue.rs:108-264 |
| 40 | med | med | concurrency | Watcher callback can fire for an old directory after navigate | panel.rs:1163-1171,890-936 |
| 41 | med | high | bug | `reload_entries` cursor save/restore across filter rebuild is obscure/fragile | panel.rs:827-850 |
| 42 | low | high | bug | Non-Unix `copy_symlink` returns `Ok(())` and silently skips symlinks | transfer.rs:641-644 |
| 43 | med | med | bug | Negative float cast to `usize` without a guard in scroll math | app/file_list.rs:189 |
| 44 | med | high | error-handling | `nsstring()` swallows `CString::new` errors from embedded NULs | native_menu.rs:22-24 |
| 45 | med | high | error-handling | Reachable `unwrap` panics if ObjC class lookup/registration fails | native_menu.rs:50-51,460 |
| 46 | med | med | perf | fuzzy `is_match()` re-trims/re-lowercases the query per entry | fuzzy.rs:29-32 |
| 47 | med | med | concurrency | Non-atomic mark-pending then spawn in image preload (low real risk) | image_cache.rs:118-141 |
| 48 | low | high | api-design | `invert()` returns `Option` but is total; hides future variants | undo.rs:42-51 |
| 49 | low | high | testing-gap | `compare()` None-mtime branch is untested | sync.rs:58-68 |
| 50 | med | high | testing-gap | Empty-input CSV/Markdown listing exports are untested | listing_export.rs:126-127 |

`*` = severity adjusted from the reviewer's by manual verification (see the
corrections table). `drop*` = false positive, should not be acted on.

## Themes (clusters worth fixing together)

1. **Error swallowing.** `let _ =` and ignored bools drop real failures on the
   save path (7) and the rename undo path (3, 20, 33, 35). One sweep to surface
   these via the existing toast system closes most of them.
2. **The dir-size index is unsafe shared state.** The clear/spawn race (5), the
   redundant nested pool (6), and the stale watcher callback (40) all stem from
   `panel.rs` owning raw `Arc<Mutex<HashMap>>` background plumbing. This is
   exactly the `DirIndex` / `BackgroundScan` extraction
   [architecture.md](architecture.md) recommends (concurrency critic).
3. **Per-frame allocations.** `size_max` (12), per-character `FontId` clone (29),
   the shelf O(n^2) (30), the dedup clone (32), and the per-entry re-lowercasing
   (13, 46) are all cheap, isolated wins that virtualization is supposed to make
   unnecessary.
4. **Rename temp-name edge cases.** The reserved-set case mismatch (9), the
   non-atomic rollback (3), and the unbounded allocator (8) cluster around
   `rename_order.rs`/`fs_util.rs` and deserve one careful pass with tests.
5. **Reachable panics.** `lock().unwrap()` poisoning (22), the batch-rename
   `unwrap` (10), the ObjC `unwrap`s (45), and `default_keep` index-0 (14) can
   crash the UI; each should fail soft or be made provably unreachable.
