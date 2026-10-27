# Copy the pointed folder tab path — 2026-10-27

Independent adaptation of [Dolphin’s Copy Location context action](https://docs.kde.org/stable_kf6/en/dolphin/dolphin/configuring-dolphin.html), applied to folder tabs, including background tabs. No upstream code/dependency copied.

Copy this folder path reads the pointed snapshot slot rather than the active folder or selected file. It uses the existing full-path formatter, preserving spaces and escaping line controls, and the existing clipboard port/feedback flow. The queued action is handled before pane activation and returns without changing tabs, current folder or selection. Invalid slot indices produce no clipboard write.

Verification:

- New regression passed: distinct active/background paths, both panes, invalid index, unchanged active bindings/global focus and Unix newline/tab escaping.
- All **29 workspace tab tests** and **15 clipboard-filtered tests** passed. The latter includes existing clipboard outcome/feedback coverage. Full suite not rerun for this localized action.
- All-target Clippy with visual-qa and warnings denied, formatting and scoped whitespace checks passed.
- Existing native folder_tabs sequence passed all **14 checks**, including context-menu actions below the added item. Its strict zero-side-effect policy remains intact: this run does not click Copy this folder path or qualify an actual OS clipboard write. The payload/binding checks are covered by the regression and clipboard tests above. Native logs retain existing Root-navigation missing /.VolumeIcon.icns warnings.

No failed checks occurred. No commit, push or remote CI run performed.
