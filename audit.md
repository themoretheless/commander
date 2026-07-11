# Code audit: top 50 things done badly or wrong (round 4)

A critical review of the commander codebase, refreshed a third time. Round 1
ran 12 **file-scoped** reviewers. Round 2 re-ran 12 **dimension-scoped**
reviewers (panics, concurrency, egui, perf, FFI, data-integrity,
error-handling, API-invariants, resource-leaks, fs-edge, lifecycle, testing).
Round 3 ran 10 **area-scoped** reviewers (security/FFI, persistence, the queue
engine, sync/compare/dedup, small utilities, app dialogs, fresh god-object
passes, image/media, docs drift). Round 4 ran 8 more area-scoped reviewers
aimed at what all three prior rounds still hadn't opened: a line-by-line pass
through the rest of `workspace.rs` (batch rename/treemap/duplicates wiring,
then pending-op/drop-glue), a fresh pass through `panel.rs`'s facets/sort/
history/jumplist code, undo/redo *completeness* (not just correctness of the
existing undo path), keybinding/command consistency, filesystem edge cases
(symlink-to-directory, stale network mounts, exotic rename semantics), a
targeted test-coverage-gap hunt, and the `app/mod.rs`/`render.rs`/`file_list.rs`
rendering layer. Every round-4 finding was adversarially re-verified exactly
like rounds 2-3 (default to REFUTED unless the mechanism is provable by
reading the cited lines); two findings were live-reproduced with a temporary
test that was written, run, and reverted. **No round-4 finding was refuted.**
This complements [architecture.md](architecture.md) (structural debt) and
feeds [recommendation.md](recommendation.md) (the plan, including the
unscoped ideas backlog in Track E and the compact Top-500 digest in Track F).

## How to read this

Each row carries a reviewer `severity`/`confidence` and a `Src` (the round
that first found it: `r1`/`r2`/`r3`/`r4`). Medium/low-confidence rows are
reviewer-reported leads to confirm, not settled facts. `*` marks a severity
adjusted by manual/adversarial verification. Rank numbers are a fresh
synthesis every round (this table supersedes round 3's numbering entirely);
the "Below the cut" section uses plain item descriptions rather than trying to
carry stale rank numbers forward across rounds.

## Resolved after round 4

- **Top-50 #1 / D12 fixed:** `native_menu.rs` now escapes double quotes and
  backslashes before embedding a path in the Finder Get Info AppleScript
  literal, with unit tests covering the escaping helper. The round-4 table
  below remains the historical audited ranking; act on item #1 as resolved in
  current code.
- **Top-50 #2-#6 fixed in the 2026-07-11 pass:** sync actions retain stable
  source paths and directory snapshots (case collisions default to an explicit
  skip); Batch Rename retains its opening context; drag start/drop targeting is
  explicit and release outside a target cancels; text diff checks a bounded,
  flattened matrix budget before allocation. The table remains the historical
  round-4 ranking; act on items #2-#6 as resolved in current code.
- **Top-50 #7, #8, #34, and #42 fixed in the continuation pass:** conflict
  policies can shrink and re-budget an overflowing transfer; F2 rename has a
  path-stable undo/redo action and undoable feedback; case-only staging rolls
  back on second-step failure; treemap directory identity is part of its data
  snapshot. Treat those historical rows as resolved in current code.
- **Top-50 #9-#12, #14, #18, #19, #21, and #32 fixed in the safety pass:**
  worker/UI mutexes recover from poisoning; duplicate/batch/ObjC access is
  fallible; Move replay validates every source before starting; filter/sort
  changes re-clamp the cursor and stale cached indices are bounds-checked;
  thumbnail allocation is checked and fallible; toolbar actions cannot replace
  pending work. The below-cut stale-cursor and Gather empty-folder gaps are
  closed too. The historical rows below remain useful as provenance.

## Resolved since round 3

No source file was touched between the round-3 audit and round 4 itself (only
`audit.md`/`architecture.md`/`recommendation.md` were edited, to sync docs).
All 50 round-3 items were still open in the code as audited at the start of
round 4.

## Verification corrections (round 4, re-checked by hand/agent against source)

| Item | Reviewer said | Verified verdict |
| --- | --- | --- |
| Jump-list never validates a target still exists | med/high, "permanent dead stop, repeated bouncing" | **Downgraded to low.** The mechanism is real (`JumpList` has no prune/remove API, `go_back`/`go_forward` never check existence), but `app/file_list.rs`'s existing `DirStatus::Gone` empty-state already offers a one-click "Go up" that escapes the dead entry immediately each time it's hit — it is a recurring papercut, not the unescapable trap the initial framing suggested. |
| `rename_noreplace` has no fallback for filesystems lacking `RENAME_EXCL` | low/med, honest non-repro | **Confirmed as a real, correctly-caveated gap.** The reviewer could not force `ENOTSUP` in this sandbox (only a local FAT32 RAM disk was available, which emulates the capability); `man rename`/`getattrlist` confirm `RENAME_EXCL` is genuinely optional per-filesystem. Kept at low/med rather than raised, since modern SMB/NFS clients are widely believed to support it and only exotic/legacy network or FUSE volumes are the realistic trigger. |
| `copy_dir_native`'s per-file errors are swallowed by `copyfile()` returning success | med/high | **Confirmed, with one nuance added.** For a `Move`, `placed=false` on this path blocks source deletion, so the *source* is preserved — this is "wasted work on an unreadable file", not source data loss. The consequence (the entire partially-successful destination tree gets deleted via `cleanup_path`'s `remove_dir_all`, not just the failed file) is real and reachable, and had zero test coverage before this finding (every existing `CopyMethod::Native` + permission-denied test takes the same-volume rename fast path instead). |

See the round-2 and round-3 correction tables further down for prior rounds' corrections, which still stand.

## Top 50 (synthesis rank; `*` = severity adjusted by manual verification)

| # | Sev | Conf | Cat | Src | Issue | Location |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | high | high | security | r3 | `action_get_info` splices the raw path into an AppleScript string with no escaping; a filename containing `"` and `&` injects arbitrary AppleScript (`do shell script`), executable by a single right-click "Get Info" | native_menu.rs:101-112 |
| 2 | high | high | bug | r3 | `apply_sync` resolves `SyncAction` rows by lowercased-name lookup (first match); two case-colliding names in one panel cause a double-copy of one and a silently dropped second file | workspace.rs:1611-1631, sync.rs:96-128 |
| 3 | high | high | bug | r4 | Batch-rename studio re-derives its target panel/selection live every frame; a single click into the other panel while the dialog is open silently retargets the whole rename to the other panel's directory and selection | workspace.rs:1306-1350, app/batch_rename_dialog.rs:11-39 |
| 4 | high | high | bug | r4 | Starting a drag on a row that is *not* part of the current keyboard selection silently drags the stale selection instead of the row under the cursor, moving/copying files the user never touched | app/file_list.rs:429-433,464-479 |
| 5 | high | high | resource-leak | r3 | `diff_lines`' O(n·m) `u32` DP matrix has no line-count cap; two ordinary text files with many short lines (well under the 2 MiB byte cap) allocate tens of GB and abort the process | textdiff.rs:20-35 |
| 6 | high | high | bug | r3 | Releasing a drag anywhere off a directory row falls back to moving the whole selection into the other panel's current directory, with no confirmation and no cancel path | workspace.rs:1793-1817, app/update.rs:1014-1020 |
| 7 | high | high | bug | r4 | Once a pending transfer's pre-resolution size overflows free space, every conflict-resolution button (Keep Both/Newer/Larger, Skip Existing, Overwrite All) and the Enter shortcut are disabled from a stale `need_bytes` that a chosen policy would have shrunk — the only path that could fix the verdict is the one thing disabled by it | workspace.rs:759-773, app/confirm_dialog.rs:53-62,271-284,354-356,374 |
| 8 | high | high | bug | r4 | Single-file rename (F2) never pushes an `undo::Action`; Cmd+Z afterward either silently reverts an unrelated older action or no-ops, with no toast either way | workspace.rs:1224-1276 (contrast with 1340-1346) |
| 9 | high | high | concurrency | r1 | `lock().unwrap()` panics on poisoned transfer state (many call sites) | transfer.rs:219-666 |
| 10 | high | high | error-handling | r1 | Duplicates-dialog commit indexes `s.keep[gi]` without bounds check | app/duplicates_dialog.rs:213-217 |
| 11 | high | high | error-handling | r1 | `batch_rename` commit `unwrap` can panic | app/batch_rename_dialog.rs:243 |
| 12 | high | high | error-handling | r2 | Panicking `unwrap` on ObjC class lookup at menu `show()` time | native_menu.rs:460,50 |
| 13 | high | high | bug | r1 | `gather_into_folder` leaves an orphan folder/files when a move fails | workspace.rs:1174-1194 |
| 14 | high | high | bug | r2 | Undo of Move silently skips deleted sources -> partial undo | workspace.rs:1062-1068 |
| 15 | high | high | bug | r1 | Rename rollback non-atomic; failed rollback `Err` dropped | rename_order.rs:56-64 |
| 16 | high | high | bug | r1 | Reserved-set casing mismatch yields colliding temp names | rename_order.rs:188-202 |
| 17 | high | high | perf | r3 | Preload's forward window never shrinks below 50 regardless of cached bytes; a folder of large RAW/HEIC photos pushes the keep-set over the 1 GB budget every frame, forcing a synchronous main-thread re-decode of the evicted active preview | app/preload.rs:28-34, image_cache.rs:182-213 |
| 18 | high | high | bug | r2 | Cursor not re-clamped when filter/facets change without a reload | panel.rs:1163-1170,1441-1450,1518-1530 |
| 19 | high | high | bug | r2 | `filtered_entries()` indexes `self.entries[i]` from a possibly-stale cache | panel.rs:1460-1464 |
| 20 | high* | high | bug | r2 | TOCTOU in `swap_into_place`: `exists()` not atomic with rename and diverges from `path_is_taken()` (symlink framing overstated) | transfer.rs:509-514 |
| 21 | high | high | bug | r2 | Unchecked integer overflow in image/video thumbnail pixel-buffer alloc (gated on CG decoding a huge image) | image_cache.rs:341-342,516-517 |
| 22 | high | high | error-handling | r3 | `copy_symlink` ignores `remove_file` failure, masking perm/lock errors | transfer.rs:637 |
| 23 | high | med | testing-gap | r1 | No on-disk undo round-trip integration test | undo.rs / workspace.rs |
| 24 | high | high | perf | r1 | `NameContains` re-lowercases the query per entry | query.rs:14-16,30 |
| 25 | high | high | perf | r2 | `select_by_relation` double-clones all filtered entries | workspace.rs:379-384 |
| 26 | high* | med | concurrency | r2 | Nested `fs_pool().install()` inside `spawn()` (redundant; deadlock unproven) | panel.rs:1027-1048,1054-1088 |
| 27 | med | high | data-integrity | r3 | `conflict::resolve`'s `KeepLarger` compares `FileEntry.size`, which is hardcoded to 0 for directories, so a directory-vs-file name collision is resolved on a meaningless size comparison | conflict.rs:99-104, panel.rs:215 |
| 28 | med | high | bug | r3 | `sync`/`compare`/`conflict` never check `is_dir`; folder rows are classified purely on the always-0 directory size and inode mtime, producing meaningless Identical/Differing/Newer results | sync.rs:58-68, compare.rs:83-94, conflict.rs:24-45 |
| 29 | med | high | data-integrity | r3 | `Predicate::MaxAgeDays`/`MinAgeDays` do unchecked `u64` multiplication; an 18+ digit value typed into Find panics in debug and silently wraps to a nonsense cutoff in release, then replays that way forever if saved as a smart folder | query.rs:36,43 |
| 30 | med | high | error-handling | r2 | `swap_into_place` restore failure orphans original at hidden backup path | transfer.rs:509-532 |
| 31 | med | high | data-integrity | r3 | All four JSON config stores (bookmarks, smart folders, command templates, session) discard the entire file on any single deserialization error, with no partial recovery or user-visible warning | bookmarks.rs:170-175, smart_folder.rs:44-49, cmdtemplate.rs:234-239, session.rs:70-73 |
| 32 | med | high | bug | r3 | Toolbar Copy/Move/Delete buttons are not gated on `pending_op`/`active_transfer`, unlike the identical keyboard path and the drag-drop path, letting a click clobber an in-flight confirmation | app/toolbar.rs:78-89, workspace.rs:716-737,775-780 |
| 33 | med | high | perf | r3 | Find runs its full recursive directory walk synchronously on the UI thread with no thread/progress/cancel, freezing the app until the walk completes or the 1000-result cap is hit | workspace.rs:1460-1485 |
| 34 | med | high | data-integrity | r3 | `commit_rename`'s case-only rename path (old -> hidden temp -> dest) has no rollback if the second rename fails, stranding the file under a dotfile name | workspace.rs:1263-1273 |
| 35 | med | high | bug | r3 | `select_all()` unconditionally overwrites the selection with just the filtered view, silently dropping previously-selected entries hidden by the current filter (unlike `invert_selection`, which preserves them) | panel.rs:1472-1485 |
| 36 | med | high | resource-leak | r3 | No negative-cache for images the decoder can never handle (SVG, MKV, WebM); preload retries every frame with a fresh `thread::spawn`, and the active preview re-attempts a synchronous decode every frame | image_cache.rs:50-86,103-145 |
| 37 | med | high | bug | r3 | `load_via_imageio` never reads EXIF/HEIF orientation; portrait photos (especially iPhone HEIC) render sideways or upside-down in the preview | image_cache.rs:255-391 |
| 38 | med | high | perf | r3 | `preload()` spawns one uncapped OS thread per uncached path in the keep-set with no concurrency limit; opening a folder of 60+ RAW/HEIC images fires dozens of simultaneous native decodes at once | image_cache.rs:104-145 |
| 39 | med | high | hang | r4 | `free_space()` shells out to `df` synchronously on the UI thread with no timeout; a stale/unresponsive network mount (SMB/NFS) freezes the entire app the moment Copy/Move is pressed, before the confirm dialog even shows | fs_util.rs:199-206, workspace.rs:220,716-737 |
| 40 | med | high | error-handling | r4 | `copy_dir_native`'s recursion continues past a per-file `copyfile()` error and returns success overall; the caller then treats the whole operation as failed and deletes the **entire partially-copied destination tree**, not just the one bad file — reachable via one unreadable file anywhere in a large native copy, zero test coverage | native_copy.rs:148-157, transfer.rs:334-365,471-481 |
| 41 | med | high | bug | r4 | `copy_dir_all` (used only by Duplicate) aborts the whole recursive copy the moment it meets a symlink pointing at a directory, since `std::fs::copy` rejects a dir-symlink target; live-reproduced (empty destination, sibling files not guaranteed copied) | fs_util.rs:46-57,174-182 |
| 42 | med | high | ui-correctness | r4 | Treemap dialog's title recomputes live from the active panel every frame while the tile data is a one-shot snapshot from when it opened; switching panels or navigating while the (non-modal) dialog is open shows the new folder's name over the old folder's sizes with no mismatch indicator | app/treemap_dialog.rs:13-33, workspace.rs:1433-1447 |
| 43 | med | high | bug | r4 | `drop_dragged`'s concurrency guard (`active_transfer`/`pending_op` in flight) returns before clearing `drag_entries`/`drop_target`, leaving a phantom "N items, drop here" overlay and directory-row highlight rendering indefinitely until the next full drag gesture | workspace.rs:1750-1791, app/file_list.rs:146,436-444, app/update.rs:843-900 |
| 44 | med | high | ui-correctness | r4 | Drop-target row highlight is computed from `panel.drag_entries` of whichever panel is being *rendered*, but `drag_entries` is only ever populated on the *source* panel of a drag — so the highlight never lights up during an ordinary cross-pane drag, and `take_drop_plan` never sees an explicit drop target for it either | app/file_list.rs:146,436-444,480-481 |
| 45 | med | high | error-handling | r2 | Spawned dir-size scan errors silently discarded | panel.rs:1027-1089 |
| 46 | low | high | bug | r4 | Back/Forward navigation never checks that a jump-list target still exists; a deleted/unmounted directory becomes a recurring dead stop in the trail (mitigated each time by the existing "Go up" affordance on the Gone-state screen, so not a permanent trap) | panel.rs:1390-1405, jumplist.rs (no prune/remove API) |
| 47 | low | high | bug | r4 | `Command::JumpSlot` (Cmd+1..9) silently no-ops with no toast when the bound bookmark's directory no longer exists | workspace.rs:519-525 |
| 48 | low* | high | bug | r3 | `opqueue::complete`/`fail` require `Running` and silently no-op if a job was paused mid-flight, permanently stranding it in `Paused`; currently dead code (pause/resume have no live caller), so a design gap to close before the queue panel (B3) ships, not a live bug | opqueue.rs:191-198,208-216 |
| 49 | low | high | ui-correctness | r3 | `Escape` closes both panels' previews at once instead of only the one the user is looking at, because the key check ignores `is_active` | app/render.rs:497-501,529-535,572-579 |
| 50 | low | med | bug | r4 | `rename_noreplace` (same-volume move fast path using `RENAME_EXCL`) has no fallback when the filesystem doesn't support the capability; would fail every move on such a volume, though not reproduced live (honest non-repro, likely narrow to legacy/exotic network mounts) | transfer.rs:309-332, native_copy.rs:84-92 |

## Below the cut (still open, displaced by round-4 findings)

Round 4 added 16 new confirmed/plausible defects, all ranking above the bottom
of round 3's list. To keep this a top-50, the following still-open items were
pushed out of the numbered table (some for the second time). Not fixed — just
lower priority than everything above. 3 are new round-4 findings that, despite
being confirmed, ranked below the cut on severity; the rest carry over from
rounds 1-3:

| Item | Location |
| --- | --- |
| Batch-rename dialog leaves a stale red error message on screen after the user edits the rule following a failed rename | app/batch_rename_dialog.rs:225-228,257-268 |
| Watcher callback can fire for the old dir after rapid navigation | panel.rs:890-936,1163-1171 |
| `compute_dir_sizes` clears maps before in-flight tasks finish writing | panel.rs:943-950 |
| `notify` `Arc<dyn Fn>` may be invoked after owning panel/workspace dropped | transfer.rs:207-246, panel.rs:902-927 |
| Transfer worker `JoinHandle` discarded; no join before context drop | transfer.rs:207-212 |
| `selected_or_cursor()` silently returns empty on out-of-bounds cursor | panel.rs:1518-1531 |
| Find-dialog results `ScrollArea` lacks a unique `id_salt` | app/find_dialog.rs:197-228 |
| Batch-rename preview `ScrollArea` lacks a unique `id_salt` | app/batch_rename_dialog.rs:154-175 |
| Run-command templates `ScrollArea` lacks a unique `id_salt` | app/run_command_dialog.rs:107 |
| `lock().unwrap()` poisoning panics in image_cache/confirm_dialog | image_cache.rs / confirm_dialog.rs |
| Non-atomic mark-pending then spawn in image preload | image_cache.rs:118-141 |
| `walk_log` HashMap grows unbounded with no pruning | panel.rs:60-64 |
| `dir_size_cache` grows unbounded between `flush_cache()` calls | panel.rs:28-32,1077-1082 |
| CallbackCtx stack pointer to `copyfile` callback (fragile, not a live UAF) | native_copy.rs:211-226,287-303 |
| `lock().unwrap()` inside the `copyfile` C callback can panic on poison | native_copy.rs:125-322 |
| image_cache `pending` entries never pruned on directory change | image_cache.rs:97-101,131-144 |
| `page_rows` is 0 until first render, breaking PageUp/Down pre-render | panel.rs:729,773 |
| `FontId` cloned per char per row per frame in highlight rendering | app/file_list.rs:687-698 |
| O(n^2) `Shelf::add`/`add_all` via linear `contains` | shelf.rs:17-26 |
| `navigate_to_file` does an allocating O(n) scan of `filtered_entries` per keystroke | workspace.rs:1484 |
| `filtered_entries()` allocates a fresh `Vec` on every call (~18 callers) | panel.rs:1460-1464 |
| Compare cache not cleared on toggle-off (stale generations) | app/update.rs:669-681 |
| Toast `Id` from reversed list index, unstable under coalescing | app/update.rs:924-932 |
| `speed_samples` uses O(n) `Vec::remove(0)` on the hot path | transfer.rs:139-141 |
| `group_duplicates` clones every `FileKey` into buckets | dedup.rs:47 |
| `is_valid_name` accepts control chars/newlines in filenames | rename.rs:96-98 |
| `nsstring` swallows `CString::new` errors -> empty labels | native_menu.rs:22-24 |
| `cleanup_path` partial `remove_dir_all` failure dropped, orphans source tree | transfer.rs:471-480 |
| Empty find term silently disables find/replace | rename.rs:80-84 |
| `opqueue` `get_state`/`set_state` are O(n) by `JobId` | opqueue.rs:137-139,278-282 |
| Negative float cast to `usize` in file_list size math | app/file_list.rs:189 |
| Empty-input CSV/Markdown export emits header-only output, untested | listing_export.rs:126-127 |
| `SmartFolders::remove`/`Bookmarks` rely on a uniqueness invariant enforced only by the mutating API, not `Deserialize`; a duplicate-keyed store cascades a delete/slot-assign meant for one entry to all of them (needs an externally edited file) | smart_folder.rs:34-36, bookmarks.rs:37-48,92-109 |
| Undoing "Gather into Folder" moves the files back out but never removes the now-empty folder it created (resolved: typed Ungather post-success cleanup removes only the empty folder; redo recreates it) | workspace.rs:1186-1218,1021-1034 (test at 2183-2213 originally documented the gap) |
| `select_by_mask`'s live "N matches" preview counts subtraction-only terms as matches, but Select adds zero of them — misleading the count | panel.rs:1324-1370, app/mask_dialog.rs:21-26 |
| Copy-path family commands (Copy Path/Name/Parent/URL/Shell/Relative) silently no-op with zero feedback when nothing is selected and the cursor sits on the ".." row | app/update.rs:172-196, panel.rs:1518-1531 |

## Themes (clusters worth fixing together)

1. **Reachable panics.** `lock().unwrap()` poisoning (9, 45-below-median-tier),
   the ObjC `unwrap`s (12), the `batch_rename` unwrap (11), and the unchecked
   indices (10, 19) can all crash the UI. Each should fail soft
   (`unwrap_or_else(|e| e.into_inner())`, `.get()`, `Result`) or be made
   provably unreachable. This is recommendation **D6**.
2. **Panel filter/cursor invariants.** The cursor-not-clamped (18), the silent
   empty `selected_or_cursor` (below the cut), the stale-index
   `filtered_entries` (19), and the `select_all` filtered-selection loss (35)
   all stem from `PanelState` exposing `cursor`/`entries`/`selected`/
   `filter_cache` as raw public fields that can drift out of sync. An
   `ensure_cursor_valid()` helper plus a borrowed iterator accessor closes most
   of them, and they are the strongest argument for the `ViewState`/
   encapsulation work in [architecture.md](architecture.md).
3. **The dir-size index is unsafe shared state.** The clear/spawn race, the
   nested pool (26), the stale watcher callback, and the unbounded
   `walk_log`/`dir_size_cache` (all below the cut) all live in `panel.rs`'s raw
   `Arc<Mutex<HashMap>>` background plumbing. This is exactly the `DirIndex` /
   `BackgroundScan` extraction the architecture doc recommends (recommendation
   **D5**).
4. **Destructive-op partial failure.** Non-atomic rename rollback (15), the
   orphan folder on a failed gather (13), the partial undo of Move (14), the
   orphaned backup on a failed swap restore (30), `commit_rename`'s dotfile
   stranding (34), the uncancellable drag-drop move (6), and the round-4
   `copy_dir_native`/`copy_dir_all` failures that discard more than the one
   bad file (40, 41) all leave the filesystem in a half-done or unintended
   state with no clear report or way to back out. These deserve one careful
   pass with on-disk integration tests (23).
5. **Per-frame / per-keystroke allocation.** Per-character `FontId` clone, the
   shelf O(n^2), the per-entry re-lowercasing (24), the double-clone in
   `select_by_relation` (25), the per-keystroke linear scan, and the
   fresh-`Vec`-per-call `filtered_entries` (all below the cut except 24/25) are
   cheap, isolated wins (recommendation **D4**).
6. **egui widget-Id hygiene.** Three dialog `ScrollArea`s lack an `id_salt`
   and the toast Id is index-derived (all below the cut); add stable salts to
   avoid cross-widget scroll/focus bleed.
7. **Cross-pane comparison is directory-blind.** `sync::compare`,
   `compare::classify_entry`, and `conflict::detect` (27, 28) all key off
   `FileEntry.size`/`modified` with no `is_dir` check; since every directory's
   `size` is hardcoded to 0 (`panel.rs:215`), folder-vs-folder and
   folder-vs-file comparisons in Sync/Compare/Conflict resolve on meaningless
   data. One `is_dir` guard (or reusing the existing recursive `dir_size_cache`)
   fixes all three call sites at once.
8. **The four JSON config stores share one fragile load pattern.** `bookmarks`,
   `smart_folder`, `cmdtemplate`, and `session` (31) all discard the *entire*
   file on any single deserialization error and have no post-load uniqueness
   validation (below-the-cut item). A shared `load_lenient<T>` helper with
   item-level fallback would close both gaps across all four modules at once.
9. **Unguarded interpreter/decoder trust boundaries.** The AppleScript
   injection in `action_get_info` (1) and the missing EXIF-orientation/
   negative-cache handling in the image pipeline (17, 36, 37, 38) both stem
   from the same root cause as the "Leaky ports" structural debt in
   architecture.md: external interpreters and decoders are invoked directly
   from the core with no sanitizing/validating port in front of them.
10. **Non-modal dialogs re-derive their target from live state instead of
    capturing it once.** The batch-rename retargeting bug (3) and the treemap
    header/tile desync (42) both stem from the same two facts: every dialog in
    the app is a plain `egui::Window` with no blocking backdrop (confirmed:
    zero `egui::Modal` usage anywhere), and dialogs re-derive their working
    panel/selection/directory from live `Workspace` state every frame instead
    of snapshotting it once when the dialog opens. A single click on the other
    panel silently retargets an open dialog's operation. This is a new,
    codebase-wide pattern worth fixing once (capture context at dialog-open
    time) rather than per-dialog.
11. **Undo coverage gaps were real and are now narrowed to other mutations.**
    `undo::Action` now models path-stable single Rename plus typed
    Gather/Ungather; Gather undo removes its empty folder through a transfer
    post-success action and redo recreates it. Delete-to-Trash and a
    partially-failed initial Gather still need their own dedicated integrity
    pass rather than being conflated with the now-closed empty-folder gap.
12. **Drag-and-drop state has three bugs stacked in the same plumbing.** The
    drop-target highlight reads the wrong panel's `drag_entries` so it never
    lights up (44), a stray click while a transfer/pending-op is in flight
    leaves a phantom drag overlay rendering forever (43), and (already
    tracked) releasing off a directory row silently falls back to Move with no
    cancel (6) — on top of which starting a drag on an unselected row drags
    the stale keyboard selection instead (4). All four live in
    `app/file_list.rs`'s drag state and `workspace.rs`'s
    `drop_dragged`/`take_drop_plan`, and would benefit from one focused
    rewrite (capture the actual dragged row explicitly, mirror highlight state
    across both panels) rather than four separate patches.
13. **Silent no-ops with zero user feedback are a recurring pattern.** Copy-path
    commands on an empty selection (below the cut), a stale `JumpSlot` (47),
    and (already tracked) `selected_or_cursor`'s empty return (below the cut)
    all fail with no toast/dialog, unlike the app's own established pattern
    elsewhere (e.g. `DiffFiles`'s "Select a file pair to diff" toast).

## Verification corrections (round 3, re-checked by hand/agent against source)

| Item | Reviewer said | Verified verdict |
| --- | --- | --- |
| AppleScript injection in `action_get_info` | high/high, "AppleScript/shell injection" | **Confirmed as described, not overstated.** `osascript` is invoked via `.arg()` (no shell involved at spawn time), but `osascript` itself re-parses the argument as an AppleScript program; an unescaped `"` in the interpolated path closes the string literal early and `&` plus `do shell script` let the rest of the "path" execute as arbitrary AppleScript, including shell commands. `is_valid_name` (`rename.rs:96-98`) does not reject `"` or `&`, so the payload filename is creatable through the app's own Rename UI. Reachable with a single right-click on a maliciously named file; kept at high/high. |
| Cross-pane sync/compare/conflict ignore `is_dir` | med/high (three related findings) | **Confirmed, real and broader than the reviewer's own framing.** Every directory's `FileEntry.size` is hardcoded to `0` (`panel.rs:215`); none of `sync::compare`, `compare::classify_entry`, or `conflict::detect` special-case directories, so folder-vs-folder and folder-vs-file rows are classified on a synthetic always-0 size and an inode mtime that does not reflect recursive content. Not just an exact-mtime-collision edge case as the reviewer's scenario implied — *every* folder-level Sync/Compare/Conflict classification is semantically meaningless, it just happens to only visibly matter when it changes the outcome. |
| `opqueue` pause/complete/fail stranding | med/high, "genuine lost-update" | **Downgraded to low.** The state-machine gap is real (no transition path lets a job paused-while-Running ever reach Done/Failed), but `pause()`/`resume()` have zero call sites outside `opqueue.rs`'s own tests today (`#[allow(dead_code)]`, module doc says they are "wired to the keyboard queue panel in a follow-up iteration"). Nothing in the shipped app can pause a `Running` job, so the race cannot fire yet. Real design debt to close before B3 (queue panel UI) lands, not a live bug. |
| No negative-cache for undecodable image/video formats | high/high, "resource-leak" | **Downgraded to med.** The uncapped per-frame thread-spawn and the main-thread synchronous re-decode of the active preview are both real and unbounded in *count*, but individual decode-attempt failures return fast (CoreGraphics/AVFoundation reject unsupported formats near-instantly), so this is sustained thread churn and UI jank, not the unbounded-memory-growth or multi-second-hang class of "high". |
| `SmartFolders`/`Bookmarks` duplicate-key cascade delete | low/med | **Confidence raised to high, severity stays low.** The `retain(|d| d.name != name)` bulk-delete mechanism is confirmed exactly as described, but no in-app code path can create the duplicate-name precondition today (only an externally edited/merged JSON file can) — real but low-likelihood; kept out of the top 50, see "Below the cut". |
| `recommendation.md` Track C text already stale on arrival | med/high | **Downgraded to low.** The document's own "Tracking" section already tells the reader Track C is done, which substantially defuses the "reader wastes time re-fixing it" scenario the reviewer described. Fixed directly in that pass rather than tracked as a numbered defect. |
| `architecture.md` "373 GUI-free tests" is stale | low/med | **Confidence raised to high.** Reproduced directly; after the 2026-07-11 regression additions, `cargo test` reports 424 passed, 1 ignored, 425 `#[test]` functions total. Fixed directly in `architecture.md`. |

## Verification corrections (round 2, re-checked by hand)

| Rank (round 2) | Item | Reviewer said | Verified verdict |
| --- | --- | --- | --- |
| 1 | `swap_into_place` `exists()` vs rename | high / "silent clobber, overwrites the link target" | **Real inconsistency, framing overstated.** The genuine defect is `dest.exists()` diverging from the `path_is_taken()` used elsewhere; `exists()` follows symlinks, so a broken symlink takes the no-backup branch. But `rename()` atomically *replaces* a broken symlink, so there is no data loss there. Fix for consistency + no-clobber rename; effective severity medium. |
| 2 | image/video pixel-buffer integer overflow | high / heap corruption | **Real, gated.** `w*4` and `h*bytes_per_row` are unchecked `usize` mults; only zero is checked. Triggering needs CoreGraphics to actually *decode* a pathologically large image first, which it usually refuses. Keep high as defense-in-depth. |
| 5/6 | cursor / `filtered_entries[i]` not bounds-checked | high | **Confirmed mechanism**, real defensive-hardening items. |
| 11 | undo of Move skips deleted sources | high | **Confirmed.** A partial undo restores N-1 of N silently. Real data-integrity inconsistency. |
| 18 | CallbackCtx stack pointer to `copyfile` | high / use-after-free | **Overstated.** `copyfile()` is synchronous and blocks the frame; not a live UAF. Box the ctx defensively. |
| 17 | nested `fs_pool().install()` | high / deadlock | **Real anti-pattern, "deadlock" unproven.** Low-medium: remove the nesting. |
