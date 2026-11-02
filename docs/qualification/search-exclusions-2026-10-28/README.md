# Search name exclusions — 2026-10-28

Independent adaptation of [fd exclusion controls](https://github.com/sharkdp/fd/blob/master/src/cli.rs), from the researched 99-repository cohort. No upstream code copied.

Find accepts `exclude:node_modules`, `exclude:*.bak` and quoted names such as `exclude:"build cache"`. Each mask matches an entire name without regard to case, using Commander `*`/`?` mask syntax independently of the global text mode. Multiple exclusions remove anything matching any mask. A matching ancestor below the root removes its whole subtree. The search root's own name is exempt. Path masks containing `/`, empty values and oversized compiled patterns are rejected; other glob characters are literal. Expressions, chips and saved JSON retain exclusions. Find's tooltip includes an example.

Live search filters jwalk directory entries before descending into excluded directories. Indexed search filters the relative path components before candidates are emitted. Excluded archive files are never opened. Archive member names and their ancestors are filtered before candidate emission; archive readers may still decompress excluded members before the callback, and excluded members still participate in existing archive read budgets. This is not full fd path-glob/ignore-file parity.

Regression coverage: query persistence across all text modes, invalid masks, compiled-size limits, and live/indexed searches combining content search, positive masks and five exclusions. The fixture includes case differences, Unicode `?`, quoted spaces, excluded directory descendants, excluded invalid ZIP files, kept/excluded ZIP members, and a root whose own basename matches an exclusion. The live candidate count proves excluded paths never reach the candidate stream; zero archive errors confirms excluded corrupt ZIPs are not inspected.

The initial regression exposed jwalk's synthetic root entry being filtered too. The correction exempts depth-zero entries and retains the same regression. Rejected focused/full logs are retained. No native UI interaction or capture; no commit or push.

Final validation: **2479 passed, 0 failed, 3 ignored** in the full locked suite. Both focused regressions passed. All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed.
