# Unicode BOM character selection — 2026-10-31

Independent adaptation of Far Manager Unicode BOM selection / code-page automatic mode:
https://github.com/FarGroup/FarManager/blob/master/far/FarEng.hlf.m4 (Open/Create default code page / Code pages menu).

The hex character column starts in Auto (Unicode BOM): accepted page zero recognizes UTF-8, UTF-16 LE or BE signatures, otherwise uses ASCII. Full UTF-32 signatures are rejected before UTF-16 prefix checks. Detection is signature-only, not heuristic or whole-file validation. BOM bytes are not stripped and byte positions/exports/search encoding are unchanged.

F8/manual menu choice sets explicit ownership, including choices made while reading. Later page completion and Reload preserve that choice. Auto (Unicode BOM) restores the cached accepted-header choice from any page without new I/O. Header cache belongs to the current validated snapshot; Reload clears it and recomputes it after accepting page zero. Auto reload without a BOM returns to ASCII. The Auto action is disabled until a header has been accepted; actual opening always starts at page zero.

Three regressions cover complete/truncated/embedded/unsupported signatures, all three Unicode modes, exact raw bytes/no extra work, manual override across page acceptance and Reload, initial busy F8 priority, no-BOM reload reset, plus actual Auto menu selection away from zero after replacing the backing file to prove cached snapshot ownership with preserved page/query/history/bookmark/generation.

Native HexBomEncoding opens a BE BOM + Привет, requires automatic BE decoding, switches explicitly to ASCII, then chooses Auto (Unicode BOM). Final capture requires restored Auto BE / .›П›р›и›в›е›т›, unchanged file cursor and no operation/modal/copied fixture. Native UTF-8/LE/Reload/busy variants, physical OS input and VoiceOver remain open. No heuristic/UTF-32 support or full Far parity is claimed.

Fresh native phase-12 capture passed 14/14 checks with all effects zero. Required checkpoints establish initial Auto BE and explicit ASCII before restoration. Inspected final frame shows Characters: Auto UTF-16 BE, visible FE FF signature, decoded Cyrillic with continuation markers, unchanged offset 0 and Hex bytes search mode. Native-launch binary hash verified. CUA returned running-app-not-found after successful short-lived capture; fresh artifacts establish completion.

Final validation: 2875 passed, 0 failed, 3 ignored. Clippy with warnings denied, default check, native visual-qa build, formatting and diff whitespace checks all passed.
