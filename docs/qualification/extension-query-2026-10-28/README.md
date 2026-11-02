# Extension query — 2026-10-28

Independent adaptation of extension filtering documented by [fd](https://github.com/sharkdp/fd#searching-for-a-particular-file-extension), a repository in the researched cohort. No upstream implementation copied.

Find accepts `ext:rs`, `ext:.RS` and compound literal suffixes such as `ext:tar.gz`. A dot boundary and file entry are required; comparison is case-insensitive, independent of Exact/Fuzzy/Regex mode. `.env` matches `ext:env`. Repeated predicates retain Commander AND semantics. Wildcards, empty dot components, whitespace and path separators are rejected. Parser canonicalizes the optional leading dot; compilation normalizes case even for directly deserialized predicates. Lowercase suffixes are prepared once, not allocated per candidate. Saved JSON and expression round trips retain the condition.

Validation: full suite **2473 passed, 0 failed, 3 ignored**. The new regression uses the owning compiled search matcher in all three modes and covers compound boundaries, directories, dotfiles, case, expression/JSON persistence and invalid inputs. Clippy with all targets, visual-qa and warnings denied passed after a stylistic collapsible-if correction (rejected log retained). The full suite preceded that behavior-preserving correction; a focused final run follows it. No native UI capture, commit or push.
