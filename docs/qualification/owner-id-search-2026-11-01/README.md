# Numeric owner and group search

Independent adaptation of [fd's numeric owner/group filtering and exclusions](https://github.com/sharkdp/fd/blob/master/doc/fd.1), cohort #77. No upstream implementation copied.

Find accepts `uid:501`, `gid:20`, `uid:!0` and `gid:!20`. IDs are unsigned decimal integers; repeated predicates combine through AND. Parsing validates digits/range, canonicalizes formatting and retains predicates through query expressions and saved-query JSON. These metadata comparisons do not depend on Exact/Fuzzy/Regex or case mode. Match explanations include the applied conditions.

Native Unix UID/GID are captured from the existing worker's lexical metadata, without extra metadata calls or account database lookups. Symbolic links use their own metadata, directories and special files are eligible unless another type condition excludes them. New indexes persist optional IDs and increase their serialized metadata budget; old indexes remain readable. Cached ownership stays a snapshot even after a source disappears. TAR member header ownership is kept separately from the containing archive's owner, including symlink and hard-link headers. ZIP does not claim ownership from its container or permission bits. Unknown required metadata increments skipped coverage and never satisfies exclusion.

## Validation

Final suite: **3125 passed / 3 ignored**; all six checks exit zero, external checksum oracles enabled. Visual-QA Clippy passes with -D warnings. Both native scenarios passed **16/16**, all recording native-effect counters zero; screenshots inspected. The independent verifier passes against the current executable hash.

The regression matrix covers native regular/hard/symbolic links, live and JSON-round-tripped indexes, TAR own-versus-target headers, known zero and extended header values, unsupported ZIP ownership, contradictory conditions, content/type conjunctions, retained metadata after source removal, unknown legacy fields, full-width parser/metadata values and invalid/ambiguous syntax. Existing maximum-document-size coverage includes both new full-width fields.

Native owner_search enters four real Find queries: owner/group inclusion, owner exclusion, group exclusion and native UID/GID with archives disabled. Intermediate query results/explanations and skipped counts are retained; final IDs are compared with raw fixture metadata. broken_link_search is rerun as a neighboring search regression. Each capture is tied to the current executable hash.

See checks.json, tests.log, validators.json, verification.log and native-captures for final evidence. `verify.py` independently validates captured query facts and raw owner IDs. Ordinary cargo check may retain the unrelated unused `response` warning in src/diff_find.rs:136; the visual-QA Clippy gate uses -D warnings.

Account-name resolution, Windows owner/SID support, ZIP extra ownership records and changes to file ownership are not implemented by these numeric predicates. IDs are observations, not proof of effective access or stable account identities across systems. GNU TAR's eight-byte extended UID/GID representation reserves its high marker bit; tests use the representable 63-bit boundary there, while the query model itself preserves all u64 values.
