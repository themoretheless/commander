# Filesystem identity and hard-link count in Get Info

Independent adaptation of [Midnight Commander's information panel](https://github.com/MidnightCommander/mc/blob/master/src/filemanager/info.c), cohort #40: device/inode location and link count. No upstream implementation copied.

Get Info adds **Filesystem ID** as a full-width decimal device/inode pair and **Hard links** for regular files, including the current name. Identity comes from the existing captured lexical PathIdentity; Unix link count is captured with already fetched listing metadata. Formatting/opening the card adds no object stat or content I/O. A symlink displays **Link object:** before its own ID and never presents the target's link count as its own. Directories say Not measured for the alias count. Missing fields stay unavailable, and zero is not used as an unknown sentinel.

The card body scrolls, values wrap, and heading/Close stay outside the scroll region. Counts and IDs are observations from the retained listing: refresh and reopen Info to observe changes. Inodes can be reused and device namespaces can change; these values are not permanent global identifiers or authorization to mutate a path. Hard-link count does not prove exclusive or reclaimable storage.

Listing JSON v1 gains additive nullable **device_id**, **file_id** and **hard_links** fields. Identity refers to the lexical object, including a link itself; hard_links is only exported for captured regular-file rows. Both identity components are null if the namespace pair is incomplete. Serialization preserves unsigned 64-bit values; consumers need integer-aware parsing beyond 2^53.

## Validation

- Three new regressions cover real hard-link aliases, equal-content separate copies and symlinks; refreshed versus retained counts after unlink; Info formatting after source deletion; full-width/zero/unknown facts; and JSON lexical identity/count semantics.
- Full suite **3122 passed / 3 ignored**; all six commands in checks.json zero, with external checksum oracles still enabled. Visual-QA Clippy is warning-free. Ordinary cargo check retains an unrelated unused `response` warning in src/diff_find.rs:136.
- Native **file_identity_info** opens the actual Get Info palette command for a two-name file. The Info state is compared with separately captured raw Unix metadata facts for the original, alias and symlink. Both aliases share device/inode and count 2; the symlink has a different inode.
- Native **listing_json** reruns the complete/selected export and checks the additive identity facts. Native **symlink_info** checks the existing symlink card with the new Link object ID and Not measured count. All three **14/14**, recording clipboard counts 0 / 2 / 0 respectively, other native-port effects zero. Screenshots are inspected.
- verify.py validates command results, current executable hashes, native manifests, raw reference facts, displayed identity/count and JSON regular/link semantics. Captures are stored durably under native-captures.

Windows link counts, ownership-name lookup and full Midnight Commander metadata parity are not claimed. Folder link counts are deliberately not reinterpreted as file aliases.
