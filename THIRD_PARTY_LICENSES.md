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
