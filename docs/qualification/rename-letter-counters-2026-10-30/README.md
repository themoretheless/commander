# Letter counters during batch rename — 2026-10-30

Independent adaptation of [Thunar alphabetic numbering option](https://raw.githubusercontent.com/xfce-mirror/thunar/master/plugins/thunar-sbr/thunar-sbr-enum-types.c), cohort repository #24. Only the public option list was inspected; no upstream implementation copied. Commander adds explicit lower/uppercase letter formats to its existing signed counter model, not a claim of exact upstream numbering parity.

Counter format offers Decimal (legacy default), lowercase Latin and uppercase Latin. Letter modes use bijective values 1=a, 26=z, 27=aa, 702=zz and 703=aaa. Zero is the literal 0; negative values prefix a minus sign to the corresponding positive letters. Padding is ignored and disabled for letter modes while retaining its setting for a return to Decimal. Start, signed/zero step, captured target order and before/after/counter-only positions retain existing semantics. Letter case is selected by format, independently of the earlier stem case transform.

The ordinal arithmetic remains widened to i128. Formatting uses unsigned magnitude, including i128::MIN, with no counter wraparound or fixed alphabet cap. Existing final-name validity, collision detection and execution-time sibling checks apply. Old JSON defaults to Decimal and omits the default field on save. Presets retain the selected format and Reset restores Decimal. Cached plans compare the format as part of the rule.

Six new regressions cover alphabet boundaries, signs/zero/padding, i128 extrema and largest ordinal arithmetic, position/decoration/case/descending/zero-step behavior, legacy serde, duplicate names, preset roundtrips, cache/submission changes and actual payload-preserving aa.txt rename/Undo/Redo/sibling collision refusal. The UI-rule load/reset regression now also covers a nondefault counter format.

Native RenameOptions retains all earlier checks. After scrolling to two-target numbering it selects lowercase and requires a.txt/b.md, selects uppercase and requires A.txt/B.md, then returns to Decimal and requires 01.txt/02.md before continuing target ordering/exclusion/export. Final readiness requires letter phase 9 and all earlier phases. Native UI does not submit a rename; real file effects are covered by the workspace regression.

Final locked visual-qa suite: **2646 passed, 0 failed, 3 ignored**. All-target Clippy with warnings denied, locked visual-qa build, cargo fmt --check and git diff --check passed. The first test attempt used format labels as preset names; case-insensitive duplicate-name protection correctly rejected the upper/lower label pair. Distinct fixture labels fixed the test without changing production behavior; the rejected log is retained.

Fresh CUA-launched native capture passed **16/16**, with letter phase 9 and all retained phases required by readiness. Exactly three recording clipboard writes from export tests and other native effects zero. Manifest postdates native-launch.json. The application captured and exited normally. No commit or push; the broader transfer objective remains open.

Frame visually inspected: all three Counter format choices fit, Decimal is restored, padding remains 2 and the retained notes.md→01.md preview/export controls are visible.
