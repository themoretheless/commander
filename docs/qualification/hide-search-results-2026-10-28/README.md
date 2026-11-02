# Hide and restore search results — 2026-10-28

Independent adaptation of Double Commander's [Remove from list / Show all found items](https://doublecmd.github.io/doc/en/findfiles.html), from the researched cohort. No upstream code copied.

Find rows have a `×` button to hide an unsuitable result. A hidden-count banner offers **Show all results** to restore rows in the current rank/time order. Underlying hits remain intact, so search totals and history counts still describe all matches. Hiding does not call a file operation or alter the query/saved definition. Successful new searches and invalid-root clearing reset hidden locations; dismissal also drops this transient state. This adds mouse controls, not a Delete shortcut or multiselect.

Hidden rows are keyed by SearchLocation, so hard-linked paths remain independent and duplicate archive members with different member indices remain independent. Incoming batches and completion sorting retain the keys. One shared visible order drives rendering and Enter reveal; keyboard counts omit hidden rows. Hiding closes that row's explanation and clears the display-row cursor/scroll request to prevent stale row activation. Restore also resets the display-row cursor. Hash lookups borrow locations without cloning paths for every rendered row.

Regression coverage checks hiding/restore with shared native identities, all time pivots, changed physical order and incoming hits, filtered keyboard row selection, hidden explanation closure, unchanged total counts, all-results-hidden recovery, and duplicate archive member positions. New-search reset paths were inspected in source. The first compile caught an action block inserted into the search-start function rather than the dialog; the block was moved to the dialog action dispatcher and the rejected log retained.

No native UI interaction, screenshot or layout capture was run. The state/navigation tests and compiled UI wiring do not establish a manual click/layout qualification. No commit or push.

Validation: **2495 passed, 0 failed, 3 ignored** in the full locked suite. Both new focused regressions passed. All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed.

Follow-up: native hide/restore/hide clicks and framebuffer capture have now been qualified at 1280×760 through the bundled macOS route; see [native qualification](../search-results-native-2026-10-28/README.md) for the 16 passed checks, image and CLI startup limitation.
