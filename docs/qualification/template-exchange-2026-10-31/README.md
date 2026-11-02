# Command-template bundle exchange — 2026-10-31

Independently adapts the import/export workflow documented in [Strata custom actions](https://github.com/lgse/strata/blob/main/docs/custom-actions.md). Commander uses its existing shell-template owner and a distinct, explicitly identified JSON format. This does not import Strata TOML manifests/scripts or claim runtime/menu-policy compatibility.

## Behavior

Run command → **Import / export templates** provides a bounded pasted JSON editor, explicit **Review JSON**, original command/extension preview, per-entry checkboxes and **Add selected templates**. Existing raw command identities keep their name/extensions/order; input duplicates keep the first occurrence; selected new entries append. Merge reparses the exact reviewed source and rechecks the current store. Modified source or row identities are refused. Successful rebase disables added/existing entries and preserves unchecked choices.

**Copy all templates as JSON** exports the current complete live store, independent of selection filters and pasted drafts. Version 1 identifies `commander.command-templates`; [example.json](example.json) is a small pasteable example. Unknown envelope/record fields and unsupported versions are rejected rather than silently importing execution/trust policies. Round trips preserve exact command whitespace/newlines, Unicode names and extension data.

Limits: 64 KiB input/output, 128 records, 256 Unicode scalars per nonempty one-line name, 8192 command bytes, at most 32 individually valid extensions. Commands reject empty/NUL/control characters except tab/newline. Multiline editor retains one character beyond the cap so oversize paste is rejected, not silently accepted as a valid prefix.

A candidate is saved through the existing command-template atomic store before live replacement. Save failure retains the prior live model; zero additions do not save. Import/export/review do not invoke shell/process APIs, set trust, alter the command draft, or submit Run. Existing selection binding, quoting, trust checks, output and cancellation still own execution. While the section is open, implicit Enter does not run the command; the explicit Run button remains available. The containing window scrolls with a viewport-dependent height bound.

## Validation and boundaries

**2963 tests passed, 3 ignored**, zero failed. Build, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks passed; see `checks.json` and logs. `source-sha256.json` records the verified source snapshot. Pure regressions cover exact round trips, unknown fields/version/size/validation refusal, duplicates, subset selection, stale/tampered review, current-store changes, failed persistence and no-op saves. A headless egui pointer test opens the section, clicks Review without import, explicitly clicks Add and copies the live store rather than the pasted draft.

No dedicated native interaction/capture was performed for this new section. Existing command-template persistence remains the owner: this change does not add cross-process revision conflict protection to that legacy store. Tests use a persistence callback and do not write host templates or execute the imported command strings. The initial compile diagnostic is retained in `initial-compile`; `initial-pointer-test` retains the animation-dependent test failure. The final headless interaction test disables theme animation before replaying pointer frames. The earlier green run is retained under `before-extension-token-tightening`; the final run also refuses comma/whitespace-containing individual extension records.
