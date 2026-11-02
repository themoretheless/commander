# Invalid-character controls: real egui input

The production checkbox/replacement field are extracted into one shared control helper. A pointer/text regression enables the checkbox, focuses the field, selects/replaces text with -, enters an invalid slash, clears the field to request stripping, disables the option, then attempts to type into the disabled field. It checks cached preview and submit-rule output on each release/text frame. Only initial fixture values are assigned directly; state changes under test arrive as pointer/key/text events.

The checkbox’s focus/lost-focus/click state now contributes to the dialog’s existing interaction guard, matching other rule controls. This prevents the form’s Enter handling from treating focus on this rule checkbox as an unowned submission action. QA/test geometry IDs expose the real control rectangles. The public rule format and backend transform are unchanged.

This qualifies the production helper and cached validation in headless egui, not a native window, full-sheet layout, OS input or complete dialog submission behavior. Native field/scroll/layout qualification remains open. No new upstream parity claim.

Final validation: all seven gates exited 0; visual-qa tests 3347 passed, 0 failed, 3 ignored. Default-feature focused regressions separately passed 4/4. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning. Final source hashes and all logs retained.
