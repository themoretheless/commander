# Forget an individual recent folder

Independent adaptation of [zoxide remove](https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/man/man1/zoxide-remove.1), cohort entry 78, consulted 2026-11-03. No upstream code copied.

Cmd+P Jump mode gains Forget recent folder for the selected unpinned recent-folder row. Bookmark/favorite/command rows cannot invoke it. The exact path and its frequency/recency record are removed under the same lock order used by visit recording. Other paths and the global monotonic ranking tick remain. A future visit records the path again with count one. This is removal, not a permanent exclusion rule.

The action keeps the switcher open, resets row selection, restores input focus and requests a fresh projection. The action button reports focus/lost-focus/click ownership so Enter cannot fall through to folder navigation. The normal session snapshot/persistence service saves the edited recent paths and stats immediately; a save failure leaves the edit in memory and reports that persistence failed. Bookmarks, panel back/forward trails and filesystem contents are unaffected by this action.

Pure regressions cover frequency cleanup, peer preservation, idempotence, JSON snapshot round-trip, re-visiting and exact non-UTF-8 identity (Unix). A real egui pointer/Enter test covers disabled pinned/command rows and enabled action focus ownership. These are not a native full-switcher or restart/persistence-failure UI qualification; those remain separate scope.

The first full run exposed a test focus assumption: pointer clicks do not establish persistent keyboard focus for an egui button. The Enter regression now explicitly requests the actual button response's focus before sending Enter, while the pointer path remains separate. The preliminary Clippy log separately records two unnecessary test clones, replaced by borrowed slices.

Final validation: all seven gates exited 0; tests 3364 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the existing diff_find.rs unused-response warning. verify.py confirmed the three named regressions, terminal results and matching final source hashes.
