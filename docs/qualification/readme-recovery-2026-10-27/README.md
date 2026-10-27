# README recovery — 2026-10-27

A previous editing script wrote its still-bound visual-QA source variable into
the root README after the intended Markdown edit. The same stale-variable error
occurred again in the later Hex-preview update. This was an agent editing error;
application Rust source was not replaced by this error.

The recovery started from the original README captured before this campaign,
which was byte-for-byte equal to Git HEAD. Only root-README string edits were
reconstructed from the session's recorded editing operations using a restricted
AST interpreter. No historical shell command or arbitrary Python code was run.
The invalid stale-variable writes were omitted. The reconstructed pre-corruption
snapshot exactly matches the README SHA-256 already recorded in the image
orientation qualification, independently confirming preservation of earlier
uncommitted documentation changes. Later intended feature descriptions were
then retained, including Unicode previews, transparency, Hex pages and byte search.

The restored README lists all 17 current native visual-QA scenarios, with no
missing/duplicate scenario commands and no missing local Markdown link targets.
The old malformed README is retained in rejected/README.corrupted.txt. Existing
historical qualification hashes remain untouched; README hashes from stages
after the bad write describe the historical malformed file, not the repaired
Markdown. verification.json records the recovery checks and corrected-file hash.

No application behavior was changed by this repair, so the Rust suite was not
rerun solely for the documentation. The scoped Git whitespace check passed.
