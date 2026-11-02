# Parent-folder name rename affixes

Independent adaptation of [Double Commander ancestor-name placeholders](https://doublecmd.github.io/doc/en/multirename.html), specifically path-level naming. The **Folder name** control inserts one lexical folder component as prefix or suffix, with a configurable separator and level 1–32. Level 1 is the captured containing directory, level 2 its parent. This is an affix policy, not Double Commander mask syntax or root-indexed placeholders.

The shared pure planner receives the immutable captured directory for both preview and apply; no filesystem metadata read or symlink canonicalization is added. Missing ancestor/root-only/non-UTF8 component or invalid configuration blocks the whole batch. Separator is at most 32 bytes, with controls, slash and NUL refused. Existing platform component lengths/collisions remain authoritative. Empty separator is allowed.

Folder prefix precedes manual prefix; folder suffix follows manual suffix and remains before file extension. If date affix is also enabled, date prefix comes first and date suffix last. Existing case/range/normalization/counter ordering remains. Directories keep their full dotted names. Presets save only level/separator/position, without a path; old rules default to no folder affix.

Five new regressions cover Unicode and two ancestor levels, date/manual-affix/extension composition, root/missing/invalid configuration, collision protection, legacy JSON, preset roundtrip/rejection, cached submission blocking, and actual filesystem rename with unavailable-level refusal before changes. Existing load/reset regression now exercises the folder policy. Preliminary test failure for separator controls was corrected and retained. Seven gates are recorded in checks.json. Native pointer-driven folder/date controls remain open.

Final result: all seven gates passed; 3326 tests passed, 0 failed, 3 ignored. Strict visual-qa Clippy is clean.
