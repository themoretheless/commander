# Optional CRLF/LF equivalence in text comparison

Independent adaptation of [Midnight Commander's “Strip trailing carriage return” diff option](https://github.com/MidnightCommander/mc/blob/master/po/pl.po). No upstream implementation copied.

**Ignore CRLF/LF differences** normalizes only the ending component of comparison keys. Original loaded sources remain intact. CRLF and LF then match, including mixed-ending files, while missing final newlines, bare/internal CR characters, inserted/deleted lines and substantive text changes stay distinct. The policy is independent of space/case settings, immediately recomputes retained rows/coordinates and survives Reload/Swap. Changed CRLF rows retain the visible CR marker; shared rows display their original minus-side content. No file is rewritten.

## Validation

- Three new regression tests cover mixed endings, default exact behavior, EOF, bare/internal/repeated CR, empty files, changed-line markers, all space/case combinations, retained sources, original source coordinates and Reload/Swap.
- Full suite **3102 passed / 3 ignored**; all six commands in `checks.json` exit zero.
- Native `diff_options` exercises fourteen real three-event button interactions (42 phases), including CRLF mode enable/disable/enable, all spacing/case options, Reload and Swap. Four changed pairs narrow to the sole `old`/`new` content change only after all selected policies apply. Turning ending equivalence off exposes the ending difference again.
- Native `diff_swap` checks the previous swap/field ownership/Reload workflow on the same executable. Both native scenarios pass **14/14** and issue zero recorded native-port effects.
- `verify.py` checks final binary hashes, six command outcomes, all fourteen phase snapshots, retained raw CRLF sources, final row text and source coordinates. `diff_options/frame.png` is the inspected final frame.

The existing source-loading and LCS limits still apply. This adds comparison policy, not encoding conversion, file rewriting, universal-newline parsing or full MC diff parity. Bare CR is deliberately content rather than a line delimiter.
