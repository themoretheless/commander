# Hex whole-file first/last search — 2026-10-30

Independent adaptation of [Double Commander viewer forward/backward search from file boundaries](https://doublecmd.github.io/doc/en/viewer.html), cohort #3. Find first starts at byte zero; Find last starts at the last byte of the bound snapshot. Both ignore the current page and previous hit. Ordinary next/previous continue from the resulting match. The Wrap setting remains unchanged.

Both commands use the existing worker submission, bounded chunk scanning, snapshot identity checks, cancellation, generation guard, progress and submitted-query history. No preliminary page read or separate scanner is added. Empty files return no match. Busy submissions and invalid queries cannot enqueue another search. The source describes boundary search directions; the two explicit action buttons are Commander's adaptation.

Three regressions cover repeated boundary commands in both wrap settings, next/previous continuation, busy/invalid submissions, empty files and actual egui pointer clicks on both buttons. Native button accessibility, focus, wrapping, scrolling and cancellation presentation remain pending: CUA is not exposed, no native launch was attempted, and historical captures do not qualify these controls.

Validation: locked visual-QA suite passed 2836 tests (3 ignored); Clippy with warnings denied, default locked check, visual-QA build, formatting and diff whitespace checks passed. Source and built-binary hashes accompany these logs. No commit or push performed.
