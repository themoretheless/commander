# Exact-name selection actions

[The Double Commander marking/clipboard workflow](https://doublecmd.github.io/doc/en/cmds.html) motivated this extension to Commander's existing pasted-name mode. Commander already accepted exact name lists; this milestone adds Remove and Invert alongside Add rather than introducing a duplicate clipboard reader. It is an independent implementation, not selection-file compatibility.

Open Select by mask, enable Exact names (one per line), paste names, choose Add / Remove / Invert, review N changes, and apply with the labelled button or Cmd/Ctrl+Enter. Only matching entries in the current filtered view and chosen Files/Folders/Both scope change. Other picks, other-folder memory and the persistent tab snapshot stay independent. Matching remains exact and case-sensitive, with no glob interpretation; missing names and duplicates cause no additional changes. Existing parser limits are 64 KiB and 4096 nonempty lines. Invalid input fails before mutation. Empty input does nothing in all three actions.

The preview cache now includes the operation and selection revision, preventing stale counts when selected identities change without a count change. Two new regressions cover action/count agreement, invalid-input preservation, filtered and kind scope, other-folder memory, and cache invalidation.

Native name_list_actions seeds notes.md + photo.png and a literal notes.md / README.txt / missing.txt list. Actual egui pointer events choose Remove, Invert, Invert, Add and press the matching apply button. The dialog is reopened with fixture text between operations. The previous name_list scenario is the control. This is UI event injection, not physical OS pasteboard/input qualification.

All six gates passed: build, full tests (3,255 passed / 3 ignored), strict Clippy, ordinary check, formatting and diff whitespace.

Final native runs: name_list_actions 14/14, previous name_list modal control 16/16; screenshots visually inspected and native effect counters all zero. Final records are selected by native-launch.json and name_list-launch.json. Earlier attempts remain preserved: initial selection seeding preceded listing completion; another run had unexpected field input; a later attempt stopped repainting after the final click. Final setup waits for loaded entries and requests repaint throughout the script without changing the 10-second deadline.

See checks.json, tests.log, verification.log and native-captures for authoritative validation. No dependencies changed. Previous dependency audit failures remain open. The 99-repository transfer goal remains open.
