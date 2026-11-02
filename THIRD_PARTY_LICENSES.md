# Third-party assets

## Strata theme catalog

`data/themes/strata.json` is converted from `data/themes/catalog.toml` in
[lgse/strata](https://github.com/lgse/strata), revision
`ac5ab4f22d86798a40d7ad30eb3528d56eecf137`.
Copyright (c) 2026 LGSE Ltd., MIT; see [license](data/licenses/Strata-MIT.txt).

The catalog includes Tinted Theming Base16 schemes, upstream revision
`fdca32a0d14ec80ad83a78a9ccb85592ca6cb9e1`, copyright Tinted Theming and
scheme authors. See [license](data/licenses/Tinted-Theming-MIT.txt).
Commander maps their semantic colors to egui; GTK CSS and syntax themes are
not used. Original catalog color values and palette identifiers are preserved.

## Rust EXIF metadata parser

Image metadata uses `kamadak-exif` 0.6.1 and its dependency `mutate_once` 0.1.2,
Copyright KAMADA Ken'ichi, BSD-2-Clause. See
[EXIF license](data/licenses/kamadak-exif-BSD-2-Clause.txt) and
[mutate_once license](data/licenses/mutate-once-BSD-2-Clause.txt).
The parser runs against a bounded in-memory snapshot on a preview worker.

## Bzip2 decompression

Archive support uses libbz2-rs-sys 0.2.5, a Rust translation derived from
bzip2/libbzip2. Copyright 1996-2021 Julian R Seward, 2019-2020 Federico Mena
Quintero, 2021 Micah Snyder, and 2024-2025 Trifecta Tech Foundation and contributors.
Commander does not claim authorship of that implementation. See the complete
[license and disclaimer](data/licenses/libbz2-rs-sys-bzip2-1.0.6.txt).

## Legacy SHA-1 interoperability

SHA-1 checksum calculation uses RustCrypto `sha1` 0.11.0, distributed under
the [MIT license](data/licenses/RustCrypto-sha1-MIT.txt). SHA-256 remains the
default; SHA-1 is exposed for compatibility with existing checksum lists.

## Adler-32 checksums

Streaming Adler-32 calculation uses `adler2` 2.0.1 under its
[MIT license](data/licenses/adler2-MIT.txt). This is an error-detection checksum;
SHA-256 remains the default for file digest comparison.

## Base64 checksum output

Digest byte encoding uses `base64` 0.22.1 under its
[MIT license](data/licenses/base64-MIT.txt).

SHA3-256 and SHA3-512 use RustCrypto `sha3` 0.12.0 and its `keccak` 0.2.2
and `sponge-cursor` 0.1.0 dependencies under their MIT license options:
[sha3](data/licenses/sha3-0.12.0-MIT.txt),
[keccak](data/licenses/keccak-0.2.2-MIT.txt),
[sponge-cursor](data/licenses/sponge-cursor-0.1.0-MIT.txt).

BLAKE2s-256 and BLAKE2b-512 file checks use the existing RustCrypto `blake2`
0.10.6 dependency under its [MIT license](data/licenses/blake2-0.10.6-MIT.txt).
The checksum feature adds it as a direct dependency without changing its version.

## Deflate64 ZIP decompression

ZIP method 9 decoding uses `deflate64` 0.1.12, derived from the .NET implementation, copyright .NET Foundation and Contributors and anatawa12. See its complete [MIT license](data/licenses/deflate64-0.1.12-MIT.txt). Commander does not claim authorship of the decoder.
