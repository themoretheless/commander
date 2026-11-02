# Hide selected search results — 2026-10-28

Independent adaptation of Double Commander's [Remove from list and group selection in Find](https://doublecmd.github.io/doc/en/findfiles.html). No upstream code copied.

Hide selected removes the selected visible result locations from the displayed list without touching the filesystem, raw results or search counters. It also clears selection for those locations, drops a hidden selection anchor/explanation, and clears the keyboard cursor/scroll request. Empty selection is a no-op and the button is disabled. Show all results restores rows without reselecting them and returns query focus. The existing single-row hide action and Copy paths/Copy selected semantics are retained.

The batch collects selected visible locations in one result-list pass, then updates sets; it does not repeatedly scan the entire result list for every selected row. Location keys keep unselected hard-link aliases and duplicate archive-member indexes independent. Later sorting/incoming batches preserve hiding. Result-action controls now wrap when the dialog is narrow.

The focused regression verifies a selected native path and archive member, an unselected hard-link alias and duplicate archive member, explanation/anchor/cursor cleanup, unchanged raw results/counts, empty repeat, sorting/new rows and restoration without reselection. Existing selection/copy regressions remain active. Native SearchResults QA performs all earlier selection/copy/tab checks, clicks Hide selected, requires all three fixture rows hidden and zero selected rows before permitting restoration, then restores all rows. Final readiness requires intact results/counts/tab ownership, no selection and query focus. Recording clipboard writes remain exactly two from prior copy actions; hide/restore must add no native effects.

No commit or push.

Validation: **2529 passed, 0 failed, 3 ignored** in the full locked visual-qa suite; focused batch-hide regression passed. All-target visual-qa Clippy with warnings denied, current QA debug build, formatting and scoped whitespace checks passed. Native hide-group/restore sequence passed **16/16 capture checks**, retaining every mandatory glyph and exactly the previous two recording clipboard writes (other effects zero). Default-size frame visually inspected: all three restored rows unselected, Hide selected/Copy selected disabled and query focus retained. The QA app exited after capture before accessibility attachment completed; fresh manifest/log/frame establish success. No rejected run for this feature. See [native frame](frame.png).
