# Miller column widths in the session

Independently implemented continuation of the Miller geometry inspired by [elementary Files](https://github.com/elementary/files/blob/main/src/View/Miller.vala), cohort #5. No upstream implementation copied.

Open-tab snapshots retain independent per-path widths. Restore covers active and inactive tabs through the existing atomic session envelope; CLI path overrides do not inherit saved geometry. Native path identity is preserved with platform-tagged bytes/units. Malformed optional geometry falls back locally, legacy sessions default empty, values clamp to 180–720, and histories have 64-entry / approximately 64 KiB encoding limits.

Tests include actual atomic file save/load into fresh workspace objects, per-tab isolation, CLI override, malformed metadata, last-record preference, and non-UTF-8 keys. **3209 passed, 3 ignored** in the full visual-QA test suite. **All six gates passed** (build, full tests, clippy with warnings denied, normal-feature check, formatting, diff whitespace). Results are in `checks.json`; detailed test output is in `tests.log`. `source-hashes.json` identifies the implementation under test.

No new native restart capture was performed in this milestone. Previous resize UI captures are historical evidence only. Closed-tab history across processes, ancestor keyboard/multi-selection and broader Miller/cohort parity remain open. No commit or push performed.
