# Numeric ownership in Get Info and listing JSON

Independent adaptation of [Midnight Commander's Owner/Group information](https://github.com/MidnightCommander/mc/blob/master/src/filemanager/info.c), cohort #40. MC resolves account names; this Commander increment exposes numeric UID/GID, consistent with the numeric Find predicates. No upstream implementation copied.

**Owner UID** and **Group GID** use already fetched lexical listing metadata. Regular files, folders and links retain their own observations; links display the explicit `Link object:` prefix even when their target is missing. Both the background directory enumeration and the one-object lexical observation path overwrite target-followed ownership with lexical observations. Failed lexical captures leave ownership unknown rather than taking a followed target's IDs. Hard-link raw counts in the directory path are likewise kept lexical.

Info formatting and listing JSON serialization add no filesystem or account database calls. Values remain retained snapshots after deletion; refresh/reopen observes new state. Unknown fields say Unavailable independently of known zero and full-width values. JSON v1 adds nullable owner_uid/owner_gid fields for any observed kind, without converting IDs to floats or account labels.

## Validation

Final suite: **3128 passed / 3 ignored**; all six commands exit zero, with external checksum oracles enabled. Visual-QA Clippy passes with -D warnings. The ordinary check retains the unrelated unused `response` warning at src/diff_find.rs:136. All three native scenarios passed **14/14**, clipboard counts 0/2/0 and other recording native effects zero. Screenshots inspected; the independent verifier passes against current executable and tool hashes.

Three new regressions cover zero/full-width/independent unknown fields, retained Info/JSON after deletion, native regular/directory/working-link/dangling-link observations through both listing and one-object paths. Existing lexical listing JSON regression also compares UID/GID against raw metadata. Synthetic row constructors initialize ownership as unknown.

The native file_identity_info and symlink_info workflows exercise the actual Get Info command; displayed ownership is compared against separately captured raw UID/GID. Native listing_json checks full/selected exports against raw ownership facts for every row. Current executable SHA-256 binds all captures. See checks.json, validators.json, tests.log, verification.log and native-captures for final evidence.

Only Unix numeric ownership is established here. Account-name lookup, Windows SIDs, ACLs and effective access are not inferred. No test changes real file ownership; live native cases compare same-user fixture facts, while synthetic cases cover differing/unknown values. UID/GID are namespace-local observations, not stable cross-system account identifiers.
