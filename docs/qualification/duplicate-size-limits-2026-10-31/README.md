# Duplicate size limits — 2026-10-31

Independently adapts [fclones minimum/maximum file-size selection](https://github.com/pkolaczk/fclones#features), cohort #85. No upstream implementation is copied.

## Behavior

Min bytes / Max bytes accept exact unsigned whole-byte bounds; both endpoints are inclusive. Blank fields omit their respective bound. The parser rejects negatives, signs, fractions, exponents, unit suffixes, Unicode digits, overflow and minimum > maximum. Input fields retain up to 64 characters so a pasted overlong integer remains invalid rather than being silently truncated into a valid 20-digit value. Empty files remain excluded, even for 0–0.

Draft edits do not change current proposals. **Apply size limits** is enabled only for a valid range different from the applied one; it restarts the current scope and rebuilds all group-indexed choices, retaining keep policy/sort. The caption always identifies the applied range for current results. **Reset size limits** clears drafts and, when needed, rescans all nonempty files. Rescan and scope changes retain the applied range and draft fields. Invalid drafts stay unapplied. Fresh opening uses unrestricted nonempty candidates.

Both modes filter before size grouping and hashing. Listing mode filters captured entries without extra metadata reads; recursive mode discards files outside the range during tree enumeration, before holding candidates or computing hash totals. Directory enumeration itself still visits the chosen tree. This does not introduce fclones CLI size-unit grammar, name filters or a throughput claim.

## Verification

Final checks in `checks.json` all returned zero: visual-qa build, full suite (**2905 passed, 3 ignored**), all-target Clippy with warnings denied, default cargo check, cargo fmt --check and git diff --check.

New tests cover exact/inclusive boundaries, empty/unbounded/zero ranges, u64::MAX without float conversion, reversed/invalid/overflow inputs and labels. A listed-file test deletes out-of-range files after capturing metadata: the allowed 10-byte group hashes successfully with zero issues, proving excluded files are not opened for hashing; including the deleted 64-byte files instead yields two read failures and no false group. Recursive tests independently select 10/64-byte groups, verify total/hashed counts and an empty gap range. The existing real-file rescan regression now verifies applied limits and draft persistence alongside newest retention/sorting and disappearing/empty groups.

The new native `duplicate_sizes` CI scene (53-scene matrix) uses two real groups: three 10-byte files and two 64-byte files. Pointer/key events enter min/max 64, Apply and copy the large-group proposal; enter invalid 65–64 and attempt the disabled Apply without changing results; clear minimum, set maximum 10, Apply and copy the small-group proposal; Reset, wait for both groups and copy all records. Terminal phase 46 requires exact expected paths/actions/sizes in all three recording clipboard payloads, correct applied ranges, unchanged fixture bytes and empty draft fields after Reset. Fresh capture: **14/14 checks**, clipboard 3, all other native effects zero. The frame was visually inspected: both fields/buttons, applied caption and restored groups are visible.

Fresh `duplicate_recursive` regression from the same binary: **14/14 checks**, two CSV copies, other effects zero, terminal phase 21. It retains scope toggling, newest hidden keeper and sorting with the new controls present. `recursive-frame.png` was inspected. This native regression uses unrestricted sizes; bounded recursive hashing is covered by the real-file worker tests, not by a filtered native recursive scene. Both launch hashes match the debug binary and copied app core.

The short-lived apps completed captures before launch observation reported application-not-found; fresh unique manifests establish completion. Actual OS clipboard delivery, physical input, VoiceOver and Trash/undo were not exercised. Symlink traversal, filesystem race and cancellation boundaries remain as described in the prior recursive-scan qualification. Full 99-repository parity remains open.
