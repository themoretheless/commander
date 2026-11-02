# EXIF attribution and capture-time details

Commander continuation of the Strata-inspired image metadata inspector. Added seven available fields: Artist, Copyright, Software, Taken UTC offset, Taken subsecond digits, Exposure program and Metering mode. Field definitions/formatting use the pinned [kamadak-exif tag implementation](https://github.com/kamadak/exif-rs/blob/master/src/tag.rs), without copying another application's inspector code.

Capture date/time, offset and fractional digits are displayed as separate recorded EXIF values. The inspector does not infer the camera timezone from the machine, convert the capture timestamp or turn a missing tag into an invented value. Attribution is displayed as metadata, without interpreting ownership or changing the file. Existing per-value output bounds/control handling, regular-file identity fences, prefix limit, cancellation and inspector ownership remain unchanged.

Eight focused metadata tests pass, including a distinct `+05:30` capture offset with unchanged `2026-11-02 12:34:56` and `250` subsecond digits, author/rights/software and exposure enums. The real JPEG/PNG/TIFF/WebP fixture combinations with/without EXIF also pass with the expanded 23-row metadata snapshot. Full suite: **3225 passed / 3 ignored**; all six gates passed. Strict visual-QA clippy is clean; the normal check retains the unrelated unused `response` warning in `src/diff_find.rs:136`.

Native ExifDetailsInfo and the header-only PNG control each passed **14/14** with zero external-effect calls. Both frames were inspected. The details frame visibly includes all seven new fields; the PNG control retains only Format/Header dimensions and its no-EXIF note. `verify.py` independently decodes JPEG markers/TIFF IFD values with Python's standard library, matches all 23 properties including unchanged timestamp, offset/fractional digits, attribution and numeric exposure enums, verifies the measured visible UTC-offset row, header-only absence, manifests, PNG dimensions and final source/binary/core/validator hashes. These are app events in a native window, not physical OS input qualification.

Final captures: `native-captures/1793602089268718000/exif_details_info` and `native-captures/1793602140968833000/image_header_info`. CUA attachment returned after captured processes exited; authoritative manifests were checked instead of relaunching them.

No dependencies or deny policy changed. The additional `cargo deny` run again returned **5**, with the same diagnostic set as the preserved pre-EXIF baseline: 13 existing zstd-family advisories and the libbz2-rs-sys license rejection. The global dependency audit remains failing; feature gate success does not waive it. See `dependencies.log` and the baseline comparison linked by the independent verifier.

Timezone validation/normalization or conversion, legal interpretation, metadata editing, more capture tags, HEIF/RAW native qualification and broader cohort parity remain open.
