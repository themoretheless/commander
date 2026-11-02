# Archive dotfile visibility — 2026-10-30

Independent adaptation of [fd hidden-file/directory exclusion](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. Hide dotfiles excludes any real archive member with a dot-prefixed normal component anywhere in its internal path, including descendants of hidden folders. The same rule excludes synthetic navigation folders. Default visibility remains all entries. Classification uses archive-relative OsStr bytes, with no host attribute lookup, payload read or symlink traversal. Ordinary dotted filenames remain visible; normalized . components do not count.

The constraint joins the existing Kinds cache identity and intersects path, size/depth/date/extension, type and narrowing predicates. Saved snapshots retain it, expose it in descriptions/menu search, and Reset clears it. Show all types preserves the separately presented visibility setting. Selected real IDs remain selected even when hidden, and selected-only copy continues to include them. Folder/history/sort owners remain current; a currently hidden folder can still be left with existing navigation controls.

Two regressions cover root/nested/Unicode dot components, duplicate IDs, navigation-folder omission, hidden selected copy, type/query composition, saved recall/Reset and actual checkbox pointer toggles without deselection.

ArchiveBrowser has an additional hidden fixture member src/.cache/secret.txt, selected alongside the existing off-folder member. Its pointer stage toggles the real Hide dotfiles control after stable placement. Capture readiness requires the setting enabled, both hidden member and synthetic folder absent from visible rows, hidden selection retained and the original sorting/location invariants. Native results and final checks follow below. No commit or push performed.


Fresh final native ArchiveBrowser capture passed 14/14 checks with all recorded effects zero. The inspected framebuffer shows Hide dotfiles enabled, .cache navigation removed, Copy selected (2) retained, the ordinary folders/member sorting and the bounded sheet/scroll. These are native-rendered injected pointer events. VoiceOver, physical keyboard interaction, native saved recall/Reset and multiple archive formats remain open.

The rejected first native run is retained in initial-hover-interference: automatic capture-probe hovering moved the pointer during the checkbox click. That hover is now suppressed until the three-step toggle completes; the final gate still requires both real and synthetic hidden rows omitted and hidden selection retained. Clippy's initial collapsible-if failure is retained and corrected. Historical qualification artifacts are unchanged. CUA reported the application missing after the final fast exit; fresh successful artifacts establish capture completion.

Final verification: locked visual-QA suite passed 2845 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source and binary hashes accompany the logs; the current binary matches the fresh native launch.
