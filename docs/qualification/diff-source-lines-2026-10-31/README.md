# Dual source line numbers in text diff — 2026-10-31

Adapts the optional line-number display documented by [Midnight Commander](https://github.com/MidnightCommander/mc/blob/master/doc/man/mc.1.in#L2863), cohort #40. This is an independent implementation for Commander's retained unified diff.

**Source line numbers (- / +)** is off by default and belongs to the open comparison. Two aligned columns identify the original minus and plus operands, matching the signed filenames above the diff. Equal rows have both numbers; deletions have only minus and insertions only plus. Blank cells do not invent a source line. Numbers are a separate, non-selectable gutter; the retained source text and existing line-ending diagnostics stay unchanged.

Coordinates are indexed once when the retained diff is opened, then accessed by original edit-script row ID. Compact context, expanding hidden equal stretches and difference navigation retain the same numbering. Virtualization still lays out only visible display rows. Toggling numbers performs no reads, recomparison or file writes. The index uses additional memory proportional to the bounded retained edit script; it is not a file reread or per-frame scan. Identical/error comparisons retain their existing messages.

## Verification

3019 tests passed, 3 ignored, zero failed. All six checks exit zero: visual-qa build, full suite, all-target Clippy with warnings denied, default check, formatting and diff whitespace. New coordinate coverage verifies unbalanced insertions/deletions, operand presence, independent contiguous source numbering, empty input and default-off state. Compact and distant-hunk renderer regressions now render with the gutter enabled, retaining virtualized reveal behavior.

Native `diff_navigation` passes 15/15 checks. The scene exercises button/Alt difference navigation, compact mode, exact-gap expansion, full restoration and refolding, then clicks the line-number toggle. Readiness requires it enabled and the selected last source block actually rendered. Independent assertions reconstruct both 180-line source operands from the retained 183-row edit script, validate every source-number pair, and require the last replacement's pairs `[161, null]` and `[null, 161]`. The inspected framebuffer shows both columns in compact mode. Existing context checkpoints and zero native effect-port calls remain required. Launch binary/core hashes and qualified source hashes are retained.

Native coverage uses the standard 1280×760 layout and three replacements; unbalanced numbering is model-tested. Physical OS input, extreme font/layout accessibility, editing, merging and patch application remain outside this milestone.
