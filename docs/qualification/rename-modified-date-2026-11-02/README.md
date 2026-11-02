# Captured modification-date rename affixes

Independent adaptation of the date-based multi-rename feature documented in [Double Commander](https://doublecmd.github.io/doc/en/multirename.html). No upstream implementation copied.

The UI exposes an optional UTC strftime prefix/suffix. Metadata is captured from the opening listing and keyed by original filename. Preview and application use that snapshot; reopening refreshes it. Saved presets store only format/position. Formats are bounded to 64 bytes, rendered dates to 512 bytes, and final names retain existing validation/collision checks. Missing timestamps and invalid/unrenderable formats block the entire batch.

Six new regressions cover date-to-source association after reordering; prefix/suffix, directories and extension transforms; pre-epoch fractional time and Unicode; bad formats, missing metadata and invalid target names; shared collision detection; preset persistence; same-frame UI submission blocking; actual filesystem application using the reviewed timestamp after an external mtime change, and whole-batch refusal with one missing timestamp.

`run_checks.py` records seven sequential gates in `checks.json` and their logs. Preliminary compiler logs are retained separately. No native pointer-driven date-control capture was performed for this milestone; UI interaction qualification remains open. No claim of full Double Commander mask parity or completion of all 99 repositories.

Result: all seven gates passed. Full suite: 3315 passed, 0 failed, 3 ignored. Strict Clippy ran with `-D warnings`.
