# Copy the current folder location — 2026-10-28

Independent adaptation of [Dolphin Copy Location](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/configuring-dolphin.html). Commander already offered a pointed tab's path from its context menu; this adds direct palette access to the focused pane's current location, including empty folders. The file-URL companion composes this behavior with Commander's existing URL formatter. No upstream code copied; no external application interoperability claim.

Copy current folder path and Copy current folder as file URL are independent of cursor/selection and filtering. They snapshot the folder at dispatch time into CopyText, so later pane switching cannot retarget the clipboard payload. Existing formatting handles path line controls or URL percent encoding. Existing clipboard port and success/failure feedback are reused; no native port/read behavior changed. Selection, filters, tab revision and pane contents remain untouched.

Validation:

- Full local suite: **2438 passed, 0 failed, 3 ignored**. New dispatch regression covers both focused panes, empty folders, unavailable cursor rows, space/percent/hash path components, both formatter styles, queued payload ownership after switching panes and untouched tab revision/selection. Another regression verifies a filtered-out selected file cannot replace the folder payload and its filter/pick remain intact.
- Existing formatter and clipboard success/failure regressions also ran in the full suite. This does not qualify a real OS clipboard write or downstream paste for the new actions.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native clipboard write or consumer-app paste was performed for these new commands. No commit or push.
