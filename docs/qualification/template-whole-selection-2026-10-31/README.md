# Whole-selection command-template matching — 2026-10-31

Independently adapts [Strata action whole-selection matching](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#matching) to Commander's existing extension-filtered shell templates.

## Behavior

Run command → Template name and file types → **Require every selected file to match** enables an opt-in rule. With a nonempty extension filter, the captured opening selection must be nonempty and every row must be a matching non-directory. Extension matching stays case-insensitive, accepts optional leading dots and compound suffixes, and retains the existing stem boundary. A mixed selection or a folder whose name merely ends in the right suffix is rejected. With no extension filter, no selection constraint is imposed.

Default/legacy templates keep their any-match suggestions and unrestricted manual Run behavior. Missing policy fields deserialize to false and false fields are omitted when persisting. Save/update carries the chosen policy; selecting a saved template restores it into the draft. Strict applicability feeds the normal browser; All templates can reveal and load an incompatible record for editing, but it cannot bypass the disabled Run/Enter gate. The UI names the refusal. Search cache keys now include directory classification as well as names, complete template content, query and scope.

Commander JSON exports now use **version 2**, preserving `require_all`; import supports versions 1 and 2. A version-1 record carrying an enabled new policy is refused. Existing same-command records still win during append-only import, including their policy; imported records never silently replace them. Import review displays whether whole-selection matching applies.

## Verification

**2969 tests passed, 3 ignored**, zero failed. Build, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks passed; see `checks.json` and logs.

Regressions cover all matching vs mixed files, empty selections, compound/case suffixes, directory rows, old serialized defaults, policy persistence, v2 round trip, v1 refusal/compatibility, existing-record precedence, and cache invalidation for identical names whose directory classification changes. Existing normal/template-browser/exchange tests remain green.

Native `template_browser` scene: **16/16** capture checks, framebuffer inspected. The fixture loads a strict ZIP template against an MD selection through All templates; its restored policy is true, the actual Run response is disabled, and real pointer down/up on that disabled button starts no run and creates no output. Search plus simultaneous Enter, scope switching, row selection and Clear also complete. `browser-state.json` records actual draft/store policy, response enablement and execution/output state; `native-validation.json` independently asserts them and identical binary/core/launch SHA-256. Native effect counters remain zero. Default window height was enlarged to keep the warning/footer visible; the fixture can scroll the real outer container before targeting a compact window's Run button. `initial-layout` retains the earlier green capture where the footer was partly clipped.

## Boundaries

This rule validates captured listing names/types, not fresh filesystem identities, MIME contents or executable behavior. It is not Strata manifest/runtime compatibility, MIME/kind/min/max-item policy support or a sandbox. Existing trust, quoting, immutable invocation selection, output and cancellation remain the execution owners. The native fixture injects a run-bar opening context and template model without host template/trust writes; it does not test the production opening trust decision, native policy-checkbox editing, or a positive command execution. Persistence uses the existing template store; cross-process conflict protection is unchanged. Full cohort parity remains open.
