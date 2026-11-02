# Command-template input kinds — 2026-10-31

Independently adapts [Strata action kind matching](https://github.com/lgse/strata/blob/main/docs/custom-actions.md#matching) to existing saved shell templates.

## Behavior

Run command → Template name and file types → **Apply to: Any / Files / Folders** configures a whole-selection constraint. Files requires a nonempty opening selection of non-directory rows; Folders requires nonempty all-directory rows. Mixed selections fail both. Any is the default and keeps previous behavior, including empty selection handling. Classification comes from captured listing `is_dir` values; no new path probes or canonicalization are introduced.

Kind constraints compose with selection counts and extension rules. Suggestions honor them; All templates may reveal/load a mismatched record, but it cannot bypass the actual Run/Enter gate. The UI states the mismatch. Selecting a record restores its kind into the draft, and Save/update preserves row identity/order. While metadata settings are expanded, the explicit Run button is used; implicit Enter cannot submit a command while editing these fields.

Persistent records default missing kind to Any and omit Any on serialization. Commander JSON now exports **version 4**, preserving `kind: files` or `kind: folders`; import review shows configured kinds. Versions 1–3 still accept policies they support. Non-Any kinds under old envelopes and unknown kind values are refused. Existing same-command metadata still wins during append-only import. [example.json](example.json) illustrates folder-only size inspection; the example command was not executed.

## Validation

**2977 tests passed, 3 ignored**, zero failed. Build, all-target visual-qa Clippy with warnings denied, default-feature check, formatting and whitespace checks passed; see `checks.json` and logs.

New regressions cover Any/Files/Folders positive, mixed and empty selections; composition with counts/strict extension rules; in-place metadata update/persistence round trip; legacy defaults; unknown kinds; v4 exchange; v1/v2/v3 downgrade refusal; valid v3 count-policy compatibility; cache invalidation for changed kind metadata and same-name directory classification; and All templates without execution bypass.

Native `template_browser`: **16/16** capture checks, framebuffer inspected. The captured selection is an MD file plus a real folder. A Files-only template is loaded through All templates, with default/unrestricted counts and legacy extension policy, proving that type classification independently disables the actual Run response. Pointer down/up on the disabled button creates no run/output. Search with simultaneous Enter, scope, original-row selection and Clear also complete. Actual types [false,true], kind files, default limits, restored/store policy and disabled/absent execution are recorded in `browser-state.json` and independently checked in `native-validation.json`. Native effect counters are zero; debug/core/launch hashes match and source hashes are recorded.

## Boundaries

Files means non-directory listing rows, not a guarantee of fresh regular-file identity; symbolic links retain the listing's existing classification. No MIME probing or new filesystem permission check is added. The native fixture injects opening context/template data without host template/trust writes; it covers loaded-kind enforcement, not native policy-selector editing, production opening trust decisions or positive command execution. Existing execution/trust/quoting/cancellation and persistence owners remain authoritative. This is not Strata TOML/runtime compatibility or full cohort parity.
