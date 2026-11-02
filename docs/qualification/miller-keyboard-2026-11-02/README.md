# Miller keyboard navigation

Independent continuation inspired by [elementary Files Miller navigation](https://github.com/elementary/files/blob/main/src/View/Miller.vala), cohort #5. No upstream implementation copied.

In Columns, plain Left goes to the parent and focuses the exact native child path. Plain Right descends only into the highlighted directory; files and the parent row do not launch anything. Modified arrows, other view modes, text inputs, modals and preview handling keep their existing routes. Commands use the existing workspace availability, pane-tool coverage and linked-navigation paths.

Enter/Backspace and horizontal navigation preserve Columns across directories whose remembered view is List. Ordinary List/Icon navigation retains its previous behavior. No filesystem scan or file opener is added to the input adapter.

Regression coverage: egui event consumption and modified keys, folder/file/parent rows, List/Icon routing, remembered-mode round trips, inactive pane ownership, no external requests and exact deferred non-UTF-8 parent-focus identity. The macOS filesystem rejects such raw-byte names, so that edge is exercised through the deterministic queued-listing owner without creating an invalid filename. Full gate output is recorded in `checks.json` and logs, implementation identities in `source-hashes.json`.

A new native keyboard capture has not been performed for this milestone. Existing pointer/resize captures remain historical evidence. Ancestor independent keyboard focus/multi-selection and the broader 99-repository parity scope remain open.

The initial failing modifier regression is retained in `failed-modifier-routing`. The adapter now inspects each event modifier exactly rather than egui subset matching.

Final full suite: **3212 passed, 3 ignored**. Initial filesystem-fixture failure is retained in `failed-filesystem-fixture`; final native-path regression uses the deterministic deferred owner.

All six final gates passed: build, full tests, clippy with warnings denied, normal-feature check, formatting and diff whitespace. Final source hashes match the qualified implementation.
