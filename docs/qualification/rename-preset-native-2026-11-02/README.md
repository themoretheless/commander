# Native saved-rename-preset workflow

Extends the real egui pointer/text/key scenario from 10 to 43 phases: select Photos; focus/name/rename to Album; focus/name/duplicate saved rule to Copy; set launch preference; clear; set again; Escape to close; Cmd+Shift+R to reopen. No model mutation simulates these actions. Only the pre-existing fixture setup seeds the saved Photos rule and selected files.

The scenario now acknowledges each injected step after the inner UI processes it. Previously advancing from raw_input_hook could skip actions when rendering was deferred. This change is scoped to RenamePresets. `verify.py` independently compares 33 retained observations to the seeded rule and checks each important transition, captured manifest and zero native effects. Successful receipt: native-captures/1793637509837008000, 16/16 checks. Files remain unchanged.

Failed attempts are retained: 1793637227005590000 was explicitly terminated (owned PID2702) after missing focus before name entry; 1793637322201823000 recorded Copy plus an unexpected extra character and failed the strict target predicate; 1793637367295787000 missed duplication while raw-hook phases advanced before UI processing. No passing status is claimed for them. CUA Raise/action attempts failed at its adapter; no successful raise is claimed.

The successful final framebuffer was inspected. It catches the dialog opening fade, with background visible through the sheet; this is evidence of the interaction state, not stable post-animation visual qualification. Error/collision native paths, native session restart persistence, date controls, and final post-animation layout remain open. Existing core/session regressions cover persistence/errors independently.

The final seven gates use the exact source of the successful native binary. Preliminary logs/failed receipts are preserved separately. No complete 99-project parity claim.

Final result: seven gates passed, 3319 tests passed / 3 ignored. Rebuilt binary SHA-256 equals the successful native launch receipt. Normal check retains the existing unrelated unused-variable warning in diff_find.rs; strict visual-qa Clippy is clean.
