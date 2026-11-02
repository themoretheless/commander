# Copy file digests as Base64

Independent adaptation of PeaZip’s documented hexadecimal/Base64 hash output: https://peazip.github.io/verify-checksum-hash.html, consulted 2026-11-02. No PeaZip code copied. Existing transitive base64 0.22.1 is now a direct dependency, with bundled MIT notice; no version upgrade or policy exception.

Copy Base64 encodes the selected algorithm’s digest bytes in the same byte order as its displayed hexadecimal string, using the standard padded Base64 alphabet, with no newline. It is not the Base64 encoding of the ASCII hex text. All eight algorithms are supported. Hex comparison, report output and GNU/SFV lists retain their current formats, explicitly explained by the new button’s tooltip. SHA-256 remains default. Encoding reads the completed report and does not reread files or start work.

Availability uses the existing single-result guard: cancellation, multiple/no rows, skipped/error rows or malformed/wrong-width hex refuse output. Tests cover known abc digests for every algorithm, uppercase hex, invalid Unicode/hex/length and unavailable result states.

The native checksum scene adds a real pointer click to Copy Base64 after Adler-32 comparison and both list copies (146 phases, expected 20 recording clipboard calls). verify.py checks the actual payload with Python zlib/base64 and preserves the preceding Adler-32/SHA-1 checks. OS pasteboard, native Base64 clicks for the other seven algorithms and alternate viewport sizes remain separate scope. No full PeaZip encoding/algorithm parity claim.

Final native receipt: 1793648093242289000/checksums, captured 14/14 with 20 recording clipboard writes and all other native-port effects zero; owned process terminal. Final toast-free frame visually inspected with all four copy buttons visible. Independent verify.py passed across 147 observations and matching source/captured binary hashes. All seven gates exited 0; tests 3339 passed, 0 failed, 3 ignored. Strict visual-qa Clippy passed; ordinary check retains the pre-existing diff_find.rs unused-response warning.
