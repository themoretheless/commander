# Keep folders first in name order — 2026-10-28

Independent adaptation of [Double Commander directory-sort policy](https://doublecmd.github.io/doc/en/configuration.html), including its name-ordered first group. No upstream code copied.

The palette offers Sort: folders by name first (toggle). This groups directories ahead of files and compares directories by ascending full name, independently of the chosen file column/direction. Existing natural/plain and case-sensitive name policies apply. Files retain the existing chosen sort and tie rules. Separating the groups before comparing names keeps the comparator consistent when folder metadata conflicts with file order.

Enabling the option turns folders-first on. Disabling this option leaves folders-first on, restoring sorting folders like files within the group. Turning folders-first off clears the independent-name policy and restores mixed sorting. ViewConfig carries the option through directory memory and tab view copies; left/right persisted pane settings and missing-field legacy defaults are covered separately. Default off preserves existing behavior.

Validation:

- Full local suite: **2436 passed, 0 failed, 3 ignored**. New sort regression covers six columns × two directions × natural/plain order, different folder metadata, file-order preservation and explicit mixed sorting after disabling grouping.
- Both-pane command regression verifies grouping is enabled from mixed sorting, actual file-size descending order under ascending folder names, preserved cursor/selection and other-pane settings, independent duplicated-tab view copies, tab JSON roundtrip and clearing the option when folders-first is disabled.
- Left/right pane persistence regression verifies serialized options and missing legacy-field defaults, plus the ViewConfig builder/default behavior. Actual desktop restart was not tested.
- All-target Clippy with visual-qa and warnings denied passed; formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native palette interaction/capture or actual desktop restart was run for this option. No commit or push.
