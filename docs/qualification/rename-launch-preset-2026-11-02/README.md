# Rename preset at launch

Independent implementation inspired by [Double Commander, Preset at launch](https://doublecmd.github.io/doc/en/multirename.html). **Use selected at launch** marks one saved rule; **Start empty** clears the preference. The next dialog opening loads the saved rule into the editor, selects it and requires normal preview/review/confirmation. These controls do not change the current edited rule or perform filesystem operations. Reset rule still clears the current editor only.

The optional `at_launch` boolean lives on the saved preset, so rename/update retain it, duplication does not inherit it, deletion removes it. Old arrays with no boolean deserialize unchanged and false values are omitted on serialization. Deserialization rejects multiple launch presets rather than choosing arbitrarily. Missing selection fails without changing the prior default.

Core regressions cover selection/replacement/clear/delete, rename/update/duplicate, legacy/ambiguous JSON and roundtrip. The actual session save/load regression retains the preference. Editor reset regression also checks loading the launch rule clears stale validation/error, retains target identity, rebuilds the proposal and can be reset. Seven gates are recorded in checks.json. Native pointer-driven opening/default controls were not captured in this milestone.

Result: all seven gates passed. Full suite: 3319 passed, 0 failed, 3 ignored. Focused preset/session suite: 23 passed.
