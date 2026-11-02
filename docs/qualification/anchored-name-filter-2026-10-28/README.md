# Prefix, suffix and whole-name pane filters — 2026-10-28

Independent adaptation of [fzf anchored matching](https://github.com/junegunn/fzf/blob/master/man/man1/fzf.1). No upstream code copied. ^report matches a literal name prefix, .rs$ a literal suffix, and ^README.md$ a whole name. This extends the earlier apostrophe-prefixed literal substring form; it does not implement the full extended-search grammar.

The marker-delimited text is literal and case-insensitive under the existing file-pane character fold. Leading apostrophe takes priority and lets users search actual ^/$ characters, retaining its significant-space behavior. Unquoted anchored queries trim outer whitespace like ordinary filters. Lone ^ or $ matches every name; ^$ matches only an empty name. Suffix matching searches for a match at the actual end, not merely the first matching substring. Range highlights use character positions from the same matcher.

Cache keys retain the original normalized query syntax, distinguishing all modes. Saved-query conversion escapes the literal text and uses the existing regex backend with absolute \A/\z boundaries, so dots/wildcards remain literal and a final newline cannot satisfy a whole-name query accidentally. No new persisted query enum/schema is added. Existing facet composition and excluded-selection retention remain.

Validation:

- Full local suite: **2455 passed, 0 failed, 3 ignored**. New matcher regression covers literal prefix/suffix/whole boundaries, escaped operators, dot literalness, final-newline rejection, empty anchors and Unicode character-index highlights including repeated suffix occurrences.
- Listing regression covers mode-specific row sets, absolute end-of-name behavior, quoted operator text and warm cache reuse. Live-panel regression compares anchored-filter rows with the owning search backend's converted query for ASCII filename fixtures, retains excluded picks and checks a whole-name result. This is bounded query-conversion evidence, not complete Unicode/backend parity.
- All-target Clippy with visual-qa and warnings denied passed. Formatting and scoped whitespace checks passed. No rejected run for this feature.

 No native anchored-input interaction/frame capture or actual saved-search UI workflow was run for this addition. No commit or push.
