# Absolute Unix directory exclusion masks — 2026-10-28

Independent follow-up to Double Commander Find Exclude subdirectories, which supports absolute paths ([primary documentation](https://doublecmd.github.io/doc/en/findfiles.html)). No upstream code copied.

excludedir: now accepts leading-/ Unix directory masks in addition to existing basenames and relative paths. Absolute masks are anchored to the lexical filesystem root and retain component-local */?, whole-component **, case policy and optional trailing slash. They classify directories only and never resolve symlinks. The search root itself stays exempt. Relative and basename semantics remain unchanged. Empty roots, repeated separators, dot/parent components and embedded globstars are rejected.

Native traversal and indexed native ancestry apply absolute masks to real directory paths. Archives stored below excluded native directories are not searched. Archive files themselves and synthetic container! paths are not directories: a matching absolute mask does not suppress their member search. Member paths remain relative to the archive and cannot satisfy an absolute Unix mask; use relative masks for them. Windows drive/UNC path-mask syntax is not implemented. No index field/schema changes.

Two new regressions cover absolute anchoring, zero/many-level globstar, terminal globstar, component boundaries, trailing slash, case, root exemption, outside-root and relative-member rejection, invalid syntax, all-mode expression/JSON round trips, native/JSON-index/ZIP/TAR matrices, archives below excluded directories, and matching file/virtual container paths that must remain eligible. Earlier relative/name/directory/link regressions run in the full suite.

fixture-compile.log retains an initial raw-string quoting correction in the test expression. No new UI widget/native screenshot, commit or push. No performance or archive payload-read avoidance claim. Validation: **2559 passed, 0 failed, 3 ignored** in the full locked visual-qa suite, including prior relative/basename/directory/link regressions. Both focused tests passed. All-target visual-qa Clippy with warnings denied, formatting and whitespace checks passed.
