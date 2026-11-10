use super::*;
use crate::testutil::TempDir;

#[test]
fn unicode_bom_recognition_requires_byte_zero_and_rejects_utf32_prefix_confusion() {
    for (bytes, expected) in [
        (vec![0xef, 0xbb, 0xbf], Some(CharacterEncoding::Utf8)),
        (vec![0xff, 0xfe, 0x41, 0], Some(CharacterEncoding::Utf16Le)),
        (vec![0xfe, 0xff, 0, 0x41], Some(CharacterEncoding::Utf16Be)),
        (Vec::new(), None),
        (vec![0xef, 0xbb], None),
        (vec![0xff], None),
        (vec![0, 0xef, 0xbb, 0xbf], None),
        (vec![0xff, 0xfe, 0, 0], None),
        (vec![0, 0, 0xfe, 0xff], None),
    ] {
        assert_eq!(CharacterEncoding::from_bom(&bytes), expected);
    }
}

#[test]
fn unicode_bom_auto_selection_respects_manual_choices_busy_keys_and_reload() {
    let temp = TempDir::new();
    let path = temp.path().join("bom-auto.bin");
    for (bom, encoding, body) in [
        (
            vec![0xef, 0xbb, 0xbf],
            CharacterEncoding::Utf8,
            b"ABC".to_vec(),
        ),
        (
            vec![0xff, 0xfe],
            CharacterEncoding::Utf16Le,
            vec![0x41, 0, 0x42, 0],
        ),
        (
            vec![0xfe, 0xff],
            CharacterEncoding::Utf16Be,
            vec![0, 0x41, 0, 0x42],
        ),
    ] {
        let mut bytes = bom;
        bytes.extend(body);
        std::fs::write(&path, &bytes).unwrap();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.character_encoding, encoding);
        assert!(!viewer.character_encoding_explicit);
        assert_eq!(viewer.detected_bom, Some(encoding));
        assert_eq!(viewer.page.as_ref().unwrap().bytes, bytes);
        assert_eq!(viewer.search_mode, crate::hex_search::InputMode::Hex);
        assert!(!runtime.run_next());
        viewer.select_character_encoding(CharacterEncoding::Ascii);
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.character_encoding, CharacterEncoding::Ascii);
        viewer.use_bom_encoding();
        assert_eq!(viewer.character_encoding, encoding);
        assert!(!runtime.run_next());
        std::fs::write(&path, b"plain").unwrap();
        viewer.reload(&ctx, &runtime.handle());
        assert!(!viewer.bom_known);
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.character_encoding, CharacterEncoding::Ascii);
        assert!(viewer.bom_known);
        assert!(viewer.detected_bom.is_none());
        viewer.select_character_encoding(CharacterEncoding::Windows1251);
        std::fs::write(&path, &bytes).unwrap();
        viewer.reload(&ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.character_encoding, CharacterEncoding::Windows1251);
        assert_eq!(viewer.detected_bom, Some(encoding));
        let mut pending = HexViewer::new(path.clone());
        pending.request(0, &ctx, &runtime.handle());
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::F8,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |_| {
                let _ = pending.show(&ctx, &runtime.handle());
            },
        ));
        assert!(runtime.run_next());
        pending.poll();
        assert_eq!(pending.character_encoding, CharacterEncoding::Windows1251);
        assert!(pending.character_encoding_explicit);
        assert_eq!(pending.detected_bom, Some(encoding));
    }
}

#[test]
fn unicode_bom_menu_restores_cached_snapshot_choice_away_from_header_without_io() {
    let temp = TempDir::new();
    let path = temp.path().join("bom-menu.bin");
    let bytes = [0xfe, 0xff, 0, 0x41, 0, 0x42];
    std::fs::write(&path, bytes).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.select_character_encoding(CharacterEncoding::Ascii);
    viewer.request(4, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "42".into();
    viewer.bookmarks[0] = Some(4);
    let generation = viewer.generation;
    std::fs::write(path, b"different").unwrap();
    for frame in 0..10 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 6, 7].contains(&frame) {
            let id = if frame < 6 {
                egui::Id::new("hex-character-menu")
            } else {
                egui::Id::new(("hex-character-encoding", "Auto (Unicode BOM)"))
            };
            let rect = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 6,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    }
    assert!(!viewer.character_encoding_explicit);
    assert_eq!(viewer.character_encoding, CharacterEncoding::Utf16Be);
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4);
    assert_eq!(viewer.page.as_ref().unwrap().bytes, [0, 0x42]);
    assert_eq!(viewer.search_input, "42");
    assert_eq!(viewer.bookmarks[0], Some(4));
    assert_eq!(viewer.history.current, Some(4));
    assert_eq!(viewer.generation, generation);
    assert!(!runtime.run_next());
}

#[test]
fn character_keys_cycle_all_modes_and_preserve_focused_fields_busy_work_and_state() {
    let temp = TempDir::new();
    let path = temp.file("character-keys.bin", "ABCD");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41".into();
    viewer.bookmarks[0] = Some(0);
    for _ in 0..2 {
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1000.0),
                )),
                ..Default::default()
            },
            |_| {
                let _ = viewer.show(&ctx, &runtime.handle());
            },
        ));
    }
    let send = |viewer: &mut HexViewer, modifiers| {
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1000.0),
                )),
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key: egui::Key::F8,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            },
            |_| {
                let _ = viewer.show(&ctx, &runtime.handle());
            },
        ))
    };
    for (encoding, _) in CharacterEncoding::ALL
        .into_iter()
        .skip(1)
        .chain(std::iter::once((CharacterEncoding::Ascii, "ASCII")))
    {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("hex-search-query")));
        send(&mut viewer, egui::Modifiers::NONE);
        assert_eq!(viewer.character_encoding, encoding);
        assert!(
            ctx.memory(|m| m.has_focus(egui::Id::new("hex-search-query"))),
            "{encoding:?}"
        );
        assert!(!runtime.run_next());
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
        assert_eq!(viewer.search_input, "41");
        assert_eq!(viewer.bookmarks[0], Some(0));
        assert_eq!(viewer.history.current, Some(0));
    }
    for modifiers in [
        egui::Modifiers::CTRL,
        egui::Modifiers::ALT,
        egui::Modifiers {
            command: true,
            mac_cmd: true,
            ..Default::default()
        },
    ] {
        send(&mut viewer, modifiers);
        assert_eq!(viewer.character_encoding, CharacterEncoding::Ascii);
    }
    egui::Popup::open_id(&ctx, egui::Id::new("character-foreign-popup"));
    send(&mut viewer, egui::Modifiers::NONE);
    assert_eq!(viewer.character_encoding, CharacterEncoding::Ascii);
    egui::Popup::close_all(&ctx);
    viewer.request(2, &ctx, &runtime.handle());
    let generation = viewer.generation;
    send(&mut viewer, egui::Modifiers::NONE);
    assert_eq!(viewer.character_encoding, CharacterEncoding::Windows1251);
    assert_eq!(viewer.generation, generation);
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 2);
    assert!(!runtime.run_next());
    send(&mut viewer, egui::Modifiers::SHIFT);
    assert!(egui::Popup::is_any_open(&ctx));
    assert_eq!(viewer.character_encoding, CharacterEncoding::Windows1251);
    assert!(!runtime.run_next());
}

#[test]
fn shift_f8_opens_character_menu_for_selection_without_new_reads() {
    let temp = TempDir::new();
    let path = temp.file("character-menu-key.bin", "ABC");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    for frame in 0..10 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if frame == 3 || frame == 4 {
            input.events = vec![
                egui::Event::ModifiersChanged(egui::Modifiers::SHIFT),
                egui::Event::Key {
                    key: egui::Key::F8,
                    physical_key: None,
                    pressed: frame == 3,
                    repeat: false,
                    modifiers: egui::Modifiers::SHIFT,
                },
            ];
        }
        if frame == 6 || frame == 7 {
            let rect = ctx
                .data(|d| {
                    d.get_temp::<egui::Rect>(egui::Id::new(("hex-character-encoding", "UTF-8")))
                })
                .unwrap();
            input.events = vec![
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 6,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    }
    assert_eq!(viewer.character_encoding, CharacterEncoding::Utf8);
    assert!(!egui::Popup::is_any_open(&ctx));
    assert!(!runtime.run_next());
    assert_eq!(viewer.page.as_ref().unwrap().bytes, b"ABC");
}

#[test]
fn utf16_character_columns_preserve_absolute_alignment_surrogates_and_page_edges() {
    let temp = TempDir::new();
    let path = temp.path().join("utf16-characters.bin");
    for encoding in [CharacterEncoding::Utf16Le, CharacterEncoding::Utf16Be] {
        let encode = |units: &[u16]| {
            units
                .iter()
                .flat_map(|u| {
                    if encoding == CharacterEncoding::Utf16Le {
                        u.to_le_bytes()
                    } else {
                        u.to_be_bytes()
                    }
                })
                .collect::<Vec<_>>()
        };
        let mut units = vec![u16::from(b'x'); 2047];
        units.extend("🌍Яé".encode_utf16());
        let bytes = encode(&units);
        std::fs::write(&path, &bytes).unwrap();
        for offset in [
            0, 4093, 4094, 4095, 4096, 4097, 4098, 4099, 4100, 4101, 4102,
        ] {
            let page = read_page(&path, offset, None, &|| false).unwrap();
            assert_eq!(
                page.bytes,
                bytes[offset as usize..(offset as usize + PAGE_BYTES).min(bytes.len())]
            );
            assert!(page.character_context.len() <= PAGE_BYTES + 6);
            for width in [1, 3, 16, 64] {
                let output = page.text_in_encoding(ByteFormat::Hex, RowWidth::new(width), encoding);
                let column: String = output
                    .lines()
                    .map(|line| line.split_once(" | ").unwrap().1)
                    .collect();
                let expected: String = (offset as usize
                    ..(offset as usize + PAGE_BYTES).min(bytes.len()))
                    .map(|index| match index {
                        4094 => '🌍',
                        4095..=4097 => '›',
                        4098 => 'Я',
                        4099 => '›',
                        4100 => 'é',
                        4101 => '›',
                        _ => {
                            if index % 2 == 0 {
                                'x'
                            } else {
                                '›'
                            }
                        }
                    })
                    .collect();
                assert_eq!(
                    column, expected,
                    "{encoding:?} offset {offset} width {width}"
                );
                assert_eq!(column.chars().count(), page.bytes.len());
            }
        }
        for (units, expected) in [
            (vec![0xd800], ".."),
            (vec![0xdc00], ".."),
            (vec![0xd800, 0x41], "..A›"),
            (vec![0xfeff, 0x202e, 0x301], ".›.›.›"),
        ] {
            std::fs::write(&path, encode(&units)).unwrap();
            let page = read_page(&path, 0, None, &|| false).unwrap();
            let row = page.row_in_encoding(0, ByteFormat::Hex, RowWidth::default(), encoding);
            assert_eq!(row.split_once(" | ").unwrap().1, expected);
        }
        let mut odd = encode(&[0x41]);
        odd.push(0xff);
        std::fs::write(&path, odd).unwrap();
        let page = read_page(&path, 0, None, &|| false).unwrap();
        assert!(
            page.row_in_encoding(0, ByteFormat::Hex, RowWidth::default(), encoding)
                .ends_with(" | A›.")
        );
    }
}

#[test]
fn utf16_character_menu_copy_keeps_raw_bytes_and_search_mode_independent() {
    let temp = TempDir::new();
    let path = temp.path().join("utf16-menu.bin");
    for (encoding, label, little) in [
        (CharacterEncoding::Utf16Le, "UTF-16 LE", true),
        (CharacterEncoding::Utf16Be, "UTF-16 BE", false),
    ] {
        let bytes: Vec<_> = "AПривет🌍Z"
            .encode_utf16()
            .flat_map(|u| {
                if little {
                    u.to_le_bytes()
                } else {
                    u.to_be_bytes()
                }
            })
            .collect();
        std::fs::write(&path, &bytes).unwrap();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        let generation = viewer.generation;
        let mut copied = None;
        for frame in 0..16 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1000.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 6, 7, 11, 12].contains(&frame) {
                let id = if frame < 6 {
                    egui::Id::new("hex-character-menu")
                } else if frame < 11 {
                    egui::Id::new(("hex-character-encoding", label))
                } else {
                    egui::Id::new("hex-copy-page")
                };
                let rect = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: [3, 6, 11].contains(&frame),
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                let (_, text) = viewer.show(&ctx, &runtime.handle());
                if text.is_some() {
                    copied = text;
                }
            }));
        }
        assert_eq!(viewer.character_encoding, encoding);
        let copied = copied.unwrap();
        assert!(copied.contains(" | A›П›р›и›в›е›т›🌍›"));
        assert!(copied.ends_with(" | ››Z›"));
        assert_eq!(viewer.page.as_ref().unwrap().bytes, bytes);
        assert_eq!(viewer.search_mode, crate::hex_search::InputMode::Hex);
        assert_eq!(viewer.generation, generation);
        assert!(!runtime.run_next());
    }
}

#[test]
fn utf8_character_column_tracks_bytes_across_rows_and_pages_with_strict_validation() {
    let temp = TempDir::new();
    let path = temp.path().join("utf8-column.bin");
    let mut bytes = vec![b'x'; 4095];
    bytes.extend_from_slice("🌍Яé".as_bytes());
    std::fs::write(&path, &bytes).unwrap();
    for offset in [
        0,
        4094,
        4095,
        4096,
        4097,
        4098,
        4099,
        4100,
        4101,
        4102,
        bytes.len() as u64,
    ] {
        let page = read_page(&path, offset, None, &|| false).unwrap();
        assert_eq!(
            page.bytes,
            bytes[offset as usize..(offset as usize + PAGE_BYTES).min(bytes.len())]
        );
        assert!(page.character_context.len() <= PAGE_BYTES + 6);
        for width in [1, 3, 16, 64] {
            let width = RowWidth::new(width);
            let rendered = page.text_in_encoding(ByteFormat::Hex, width, CharacterEncoding::Utf8);
            let actual: String = rendered
                .lines()
                .map(|line| line.split_once(" | ").unwrap().1)
                .collect();
            let expected: String = (offset as usize
                ..(offset as usize + PAGE_BYTES).min(bytes.len()))
                .map(|index| match index {
                    4095 => '🌍',
                    4096..=4098 => '›',
                    4099 => 'Я',
                    4100 => '›',
                    4101 => 'é',
                    4102 => '›',
                    _ => 'x',
                })
                .collect();
            assert_eq!(actual, expected, "offset {offset}, width {}", width.bytes());
            assert_eq!(actual.chars().count(), page.bytes.len());
        }
    }
    for invalid in [
        vec![0xc0, 0xaf],
        vec![0xed, 0xa0, 0x80],
        vec![0xf4, 0x90, 0x80, 0x80],
        vec![0xf0, 0x9f, 0x8c],
        vec![0x80, 0xbf],
        vec![0xe2, 0x28, 0xa1],
    ] {
        std::fs::write(&path, &invalid).unwrap();
        let page = read_page(&path, 0, None, &|| false).unwrap();
        let column: String = (0..invalid.len())
            .map(|i| CharacterEncoding::Utf8.glyph_at(&page.character_context, i))
            .collect();
        assert_eq!(
            column,
            if invalid.contains(&b'(') {
                ".(.".to_owned()
            } else {
                ".".repeat(invalid.len())
            }
        );
    }
    let controls = "A\n\u{202e}\u{200d}\u{301}Z".as_bytes();
    std::fs::write(&path, controls).unwrap();
    let page = read_page(&path, 0, None, &|| false).unwrap();
    let column: String = (0..controls.len())
        .map(|i| CharacterEncoding::Utf8.glyph_at(&page.character_context, i))
        .collect();
    assert_eq!(column, "A..››.››.›Z");
}

#[test]
fn utf8_character_menu_copy_preserves_snapshot_and_raw_export() {
    let temp = TempDir::new();
    let path = temp.file("utf8-menu.bin", "AПривет🌍Z");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41".into();
    let generation = viewer.generation;
    let mut copied = None;
    for frame in 0..16 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 6, 7, 11, 12].contains(&frame) {
            let id = if frame < 6 {
                egui::Id::new("hex-character-menu")
            } else if frame < 11 {
                egui::Id::new(("hex-character-encoding", "UTF-8"))
            } else {
                egui::Id::new("hex-copy-page")
            };
            let rect = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: [3, 6, 11].contains(&frame),
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let (_, text) = viewer.show(&ctx, &runtime.handle());
            if text.is_some() {
                copied = text;
            }
        }));
    }
    assert_eq!(viewer.character_encoding, CharacterEncoding::Utf8);
    let copied = copied.unwrap();
    assert!(copied.contains(" | AП›р›и›в›е›т›🌍››"));
    assert!(copied.ends_with(" | ›Z"));
    assert_eq!(
        std::str::from_utf8(&viewer.page.as_ref().unwrap().bytes).unwrap(),
        "AПривет🌍Z"
    );
    assert_eq!(viewer.generation, generation);
    assert!(!runtime.run_next());
    assert_eq!(viewer.search_input, "41");
}

#[test]
fn single_byte_character_columns_decode_cyrillic_and_keep_raw_bytes_and_alignment() {
    for (encoding, bytes, text) in [
        (
            CharacterEncoding::Windows1251,
            vec![0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2],
            "Привет",
        ),
        (
            CharacterEncoding::Koi8R,
            vec![0xf0, 0xd2, 0xc9, 0xd7, 0xc5, 0xd4],
            "Привет",
        ),
        (
            CharacterEncoding::Koi8U,
            crate::hex_search::InputMode::Koi8U.parse("Привіт").unwrap(),
            "Привіт",
        ),
        (
            CharacterEncoding::Ibm866,
            vec![0x8f, 0xe0, 0xa8, 0xa2, 0xa5, 0xe2],
            "Привет",
        ),
        (CharacterEncoding::Windows1252, vec![0x80, 0xe9], "€é"),
    ] {
        assert_eq!(
            bytes.iter().map(|b| encoding.glyph(*b)).collect::<String>(),
            text
        );
    }
    let temp = TempDir::new();
    let path = temp.path().join("all-characters.bin");
    let bytes: Vec<_> = (0..=255).collect();
    std::fs::write(&path, &bytes).unwrap();
    let page = read_page(&path, 0, None, &|| false).unwrap();
    for (encoding, _) in CharacterEncoding::ALL {
        for width in [1, 17, 64] {
            let width = RowWidth::new(width);
            for format in [ByteFormat::Hex, ByteFormat::Decimal] {
                let encoded = page.text_in_encoding(format, width, encoding);
                let ascii = page.text_in_encoding(format, width, CharacterEncoding::Ascii);
                for (line, baseline) in encoded.lines().zip(ascii.lines()) {
                    let (raw, chars) = line.split_once(" | ").unwrap();
                    assert_eq!(raw, baseline.split_once(" | ").unwrap().0);
                    assert_eq!(
                        chars.chars().count(),
                        baseline.split_once(" | ").unwrap().1.len()
                    );
                    assert!(!chars.chars().any(char::is_control));
                    assert!(!chars.contains(char::REPLACEMENT_CHARACTER));
                    assert!(!chars.contains('\u{ad}'));
                }
            }
        }
    }
    assert_eq!(CharacterEncoding::Windows1252.glyph(0x81), '.');
    assert_eq!(CharacterEncoding::Windows1251.glyph(0xa0), '.');
    assert_eq!(CharacterEncoding::Ascii.glyph(0xff), '.');
}

#[test]
fn character_encoding_menu_and_copy_page_use_same_snapshot_without_work() {
    let temp = TempDir::new();
    let path = temp.path().join("characters.bin");
    let bytes = [0xcf, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2];
    std::fs::write(&path, bytes).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    viewer.bookmarks[0] = Some(0);
    let generation = viewer.generation;
    let mut copied = None;
    for frame in 0..16 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 1000.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 6, 7, 11, 12].contains(&frame) {
            let id = if frame < 6 {
                egui::Id::new("hex-character-menu")
            } else if frame < 11 {
                egui::Id::new(("hex-character-encoding", "Windows-1251"))
            } else {
                egui::Id::new("hex-copy-page")
            };
            let rect = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: [3, 6, 11].contains(&frame),
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let (_, text) = viewer.show(&ctx, &runtime.handle());
            if text.is_some() {
                copied = text;
            }
        }));
    }
    assert_eq!(viewer.character_encoding, CharacterEncoding::Windows1251);
    assert!(copied.unwrap().ends_with(" | Привет"));
    assert_eq!(
        viewer.page.as_ref().unwrap().byte_text(ByteFormat::Hex),
        "CF F0 E8 E2 E5 F2"
    );
    assert_eq!(viewer.generation, generation);
    assert!(!runtime.run_next());
    assert_eq!(viewer.search_input, "41 42");
    assert_eq!(viewer.search_mode, crate::hex_search::InputMode::Hex);
    assert_eq!(viewer.bookmarks[0], Some(0));
    assert_eq!(viewer.history.current, Some(0));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn position_percent_is_exact_at_zero_eof_and_u64_limits_without_early_roundup() {
    for (offset, size, expected) in [
        (0, 0, "n/a"),
        (0, 3, "0.00%"),
        (1, 3, "33.33%"),
        (2, 3, "66.66%"),
        (3, 3, "100.00%"),
        (1, 10001, "0.00%"),
        (10000, 10001, "99.99%"),
        (u64::MAX / 2, u64::MAX, "49.99%"),
        (u64::MAX / 2 + 1, u64::MAX, "50.00%"),
        (u64::MAX - 1, u64::MAX, "99.99%"),
        (u64::MAX, u64::MAX, "100.00%"),
        (u64::MAX, 1, "100.00%"),
    ] {
        assert_eq!(position_percent(offset, size), expected, "{offset}/{size}");
    }
}

#[test]
fn position_status_uses_loaded_page_start_and_snapshot_size() {
    let temp = TempDir::new();
    let path = temp.path().join("position-status.bin");
    std::fs::write(&path, [0; 6]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(3, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |_| {
        let _ = viewer.show(&ctx, &runtime.handle());
    }));
    let status = ctx
        .data(|data| data.get_temp::<String>(egui::Id::new("hex-position-status")))
        .unwrap();
    assert_eq!(
        status,
        "Offset 0x3 (3) · Position 50.00% · 3 bytes shown · 6 bytes in file"
    );
    assert!(!runtime.run_next());
    assert_eq!(viewer.history.current, Some(3));
}

#[test]
fn row_width_layout_roundtrips_every_byte_and_exact_offset_for_all_widths() {
    let temp = TempDir::new();
    let path = temp.path().join("row-width.bin");
    let bytes: Vec<u8> = (0..4096).map(|n| (n % 256) as u8).collect();
    std::fs::write(&path, &bytes).unwrap();
    let page = read_page(&path, 7, None, &|| false).unwrap();
    assert_eq!(RowWidth::new(0).bytes(), 1);
    assert_eq!(RowWidth::new(255).bytes(), 64);
    for count in 1..=64 {
        let width = RowWidth::new(count);
        for format in [ByteFormat::Hex, ByteFormat::Decimal] {
            let text = page.text_in_layout(format, width);
            let mut decoded = Vec::new();
            let mut ascii_column = None;
            for (row, line) in text.lines().enumerate() {
                let (values, ascii) = line.split_once(" | ").unwrap();
                let column = values.len();
                assert_eq!(*ascii_column.get_or_insert(column), column);
                let mut values = values.split_whitespace();
                let offset = u64::from_str_radix(values.next().unwrap(), 16).unwrap();
                assert_eq!(offset, 7 + (row * width.bytes()) as u64);
                let row_bytes: Vec<_> = values
                    .map(|v| {
                        u8::from_str_radix(v, if format == ByteFormat::Hex { 16 } else { 10 })
                            .unwrap()
                    })
                    .collect();
                let expected_ascii: String = row_bytes
                    .iter()
                    .map(|&b| {
                        if (0x20..=0x7e).contains(&b) {
                            char::from(b)
                        } else {
                            '.'
                        }
                    })
                    .collect();
                assert_eq!(ascii, expected_ascii);
                decoded.extend(row_bytes);
            }
            assert_eq!(decoded, bytes[7..]);
            assert_eq!(
                text.lines().count(),
                page.bytes.len().div_ceil(width.bytes())
            );
        }
    }
    std::fs::write(&path, []).unwrap();
    let empty = read_page(&path, 0, None, &|| false).unwrap();
    assert_eq!(empty.text_in_layout(ByteFormat::Hex, RowWidth::new(1)), "");
}

#[test]
fn byte_alignment_keys_preserve_width_and_guard_edges_focus_popup_and_busy() {
    let temp = TempDir::new();
    let path = temp.file("align.bin", "ABCDE");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.row_width = RowWidth::new(3);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    let send = |viewer: &mut HexViewer, key, modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    };
    send(&mut viewer, egui::Key::ArrowLeft, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
    for offset in 1..=5 {
        send(&mut viewer, egui::Key::ArrowRight, egui::Modifiers::CTRL);
        let generation = viewer.generation;
        send(&mut viewer, egui::Key::ArrowRight, egui::Modifiers::CTRL);
        assert_eq!(
            viewer.generation, generation,
            "busy key queues no second read"
        );
        assert!(runtime.run_next());
        viewer.poll();
        let page = viewer.page.as_ref().unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(page.bytes, b"ABCDE"[offset as usize..]);
        assert_eq!(viewer.row_width.bytes(), 3);
        assert_eq!(viewer.history.current, Some(offset));
    }
    send(&mut viewer, egui::Key::ArrowRight, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
    send(&mut viewer, egui::Key::ArrowLeft, egui::Modifiers::CTRL);
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().bytes, b"E");
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("hex-search-query")));
    send(&mut viewer, egui::Key::ArrowLeft, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
    ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("hex-search-query")));
    egui::Popup::open_id(&ctx, egui::Id::new("alignment-popup"));
    send(&mut viewer, egui::Key::ArrowLeft, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
    egui::Popup::close_all(&ctx);
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        egui::Modifiers {
            ctrl: true,
            mac_cmd: true,
            command: true,
            ..Default::default()
        },
    ] {
        send(&mut viewer, egui::Key::ArrowLeft, modifiers);
        assert!(!runtime.run_next());
    }
}

#[test]
fn page_navigation_keys_load_exact_blocks_and_preserve_input_focus() {
    let temp = TempDir::new();
    let path = temp.path().join("page-keys.bin");
    std::fs::write(&path, (0..=255).cycle().take(9001).collect::<Vec<u8>>()).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(4095, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    let send = |viewer: &mut HexViewer, key, modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    };
    viewer.bookmarks[0] = Some(4095);
    for (key, modifiers, expected, read) in [
        (egui::Key::PageDown, egui::Modifiers::NONE, 8191, true),
        (egui::Key::PageUp, egui::Modifiers::NONE, 4095, true),
        (egui::Key::Home, egui::Modifiers::NONE, 0, true),
        (egui::Key::PageDown, egui::Modifiers::NONE, 4096, true),
        (egui::Key::End, egui::Modifiers::CTRL, 8192, true),
        (egui::Key::PageDown, egui::Modifiers::NONE, 8192, false),
        (egui::Key::Home, egui::Modifiers::CTRL, 0, true),
        (egui::Key::PageUp, egui::Modifiers::NONE, 0, false),
    ] {
        send(&mut viewer, key, modifiers);
        if read {
            let generation = viewer.generation;
            send(&mut viewer, key, modifiers);
            assert_eq!(viewer.generation, generation);
        }
        assert_eq!(runtime.run_next(), read);
        viewer.poll();
        let page = viewer.page.as_ref().unwrap();
        assert_eq!(page.offset, expected);
        assert_eq!(page.bytes[0], (expected % 256) as u8);
        assert_eq!(viewer.history.current, Some(expected));
    }
    assert_eq!(viewer.bookmarks[0], Some(4095));
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("hex-search-query")));
    send(&mut viewer, egui::Key::End, egui::Modifiers::NONE);
    assert!(!runtime.run_next());
    ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("hex-search-query")));
    egui::Popup::open_id(&ctx, egui::Id::new("page-key-popup"));
    send(&mut viewer, egui::Key::PageDown, egui::Modifiers::NONE);
    assert!(!runtime.run_next());
    egui::Popup::close_all(&ctx);
    for modifiers in [
        egui::Modifiers::ALT,
        egui::Modifiers::SHIFT,
        egui::Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        },
    ] {
        send(&mut viewer, egui::Key::End, modifiers);
        assert!(!runtime.run_next());
    }
    send(&mut viewer, egui::Key::PageDown, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
}

#[test]
fn page_navigation_keys_handle_empty_and_exact_page_size_edges() {
    for size in [0, 4096, 4097] {
        let temp = TempDir::new();
        let path = temp.file("edge.bin", &"x".repeat(size));
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path);
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::End,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        assert_eq!(runtime.run_next(), size > 4096);
        viewer.poll();
        let page = viewer.page.as_ref().unwrap();
        assert_eq!(page.offset, if size > 4096 { 4096 } else { 0 });
        assert_eq!(page.bytes.len(), if size > 4096 { 1 } else { size });
        assert!(!runtime.run_next());
    }
}

#[test]
fn position_history_shortcuts_use_guarded_reads_and_preserve_text_focus() {
    let temp = TempDir::new();
    let path = temp.file("history-keys.bin", "0123456789");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    for offset in [0, 3, 7] {
        viewer.request(offset, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
    }
    let send = |viewer: &mut HexViewer, key, modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    };
    for (key, modifiers, expected) in [
        (egui::Key::Z, egui::Modifiers::CTRL, 3),
        (egui::Key::Backspace, egui::Modifiers::ALT, 0),
        (
            egui::Key::Z,
            egui::Modifiers {
                ctrl: true,
                shift: true,
                ..Default::default()
            },
            3,
        ),
    ] {
        send(&mut viewer, key, modifiers);
        let generation = viewer.generation;
        send(&mut viewer, key, modifiers);
        assert_eq!(viewer.generation, generation);
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.page.as_ref().unwrap().offset, expected);
        assert_eq!(
            viewer.page.as_ref().unwrap().bytes[0],
            b'0' + expected as u8
        );
        assert!(!runtime.run_next());
    }
    ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("hex-search-query")));
    send(&mut viewer, egui::Key::Z, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
    assert_eq!(viewer.page.as_ref().unwrap().offset, 3);
    ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("hex-search-query")));
    egui::Popup::open_id(&ctx, egui::Id::new("history-key-popup"));
    send(&mut viewer, egui::Key::Z, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
    egui::Popup::close_all(&ctx);
    send(
        &mut viewer,
        egui::Key::Z,
        egui::Modifiers {
            mac_cmd: true,
            command: true,
            ..Default::default()
        },
    );
    assert!(!runtime.run_next());
}

#[test]
fn row_width_shortcuts_reflow_without_reads_at_edges_and_block_boundaries() {
    let temp = TempDir::new();
    let path = temp.file("width-keys.bin", "0123456789abcdefghijklmnop");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(3, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.bookmarks[0] = Some(3);
    for (width, left, right, block_left, block_right) in [
        (1, 1, 2, 1, 16),
        (8, 7, 9, 1, 16),
        (16, 15, 17, 1, 32),
        (17, 16, 18, 16, 32),
        (31, 30, 32, 16, 32),
        (32, 31, 33, 16, 48),
        (63, 62, 64, 48, 64),
        (64, 63, 64, 48, 64),
    ] {
        for (key, ctrl, expected) in [
            (egui::Key::ArrowLeft, false, left),
            (egui::Key::ArrowRight, false, right),
            (egui::Key::ArrowLeft, true, block_left),
            (egui::Key::ArrowRight, true, block_right),
        ] {
            viewer.row_width = RowWidth::new(width);
            let modifiers = egui::Modifiers {
                alt: true,
                ctrl,
                ..Default::default()
            };
            let input = egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            };
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                let _ = viewer.show(&ctx, &runtime.handle());
            }));
            assert_eq!(
                viewer.row_width,
                RowWidth::new(expected),
                "width={width}, key={key:?}, block={ctrl}"
            );
            assert!(!runtime.run_next());
            assert_eq!(viewer.page.as_ref().unwrap().offset, 3);
            assert_eq!(viewer.history.current, Some(3));
            assert_eq!(viewer.bookmarks[0], Some(3));
        }
    }
}

#[test]
fn row_width_shortcuts_preserve_focused_input_popup_and_busy_work() {
    let temp = TempDir::new();
    let path = temp.file("width-key-guards.bin", "0123456789");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    let send = |viewer: &mut HexViewer, modifiers: egui::Modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::ArrowRight,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    };
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::CTRL,
        egui::Modifiers {
            alt: true,
            shift: true,
            ..Default::default()
        },
        egui::Modifiers {
            alt: true,
            mac_cmd: true,
            ..Default::default()
        },
    ] {
        send(&mut viewer, modifiers);
        assert_eq!(viewer.row_width.bytes(), 16);
        if modifiers == egui::Modifiers::CTRL {
            // Ctrl+Right now aligns the page; it still must not resize rows.
            assert!(runtime.run_next());
            viewer.poll();
            assert_eq!(viewer.page.as_ref().unwrap().offset, 1);
        }
    }
    for id in ["hex-search-query", "hex-offset-input"] {
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(id)));
        send(&mut viewer, egui::Modifiers::ALT);
        assert_eq!(viewer.row_width.bytes(), 16);
        ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new(id)));
    }
    egui::Popup::open_id(&ctx, egui::Id::new("width-popup"));
    send(&mut viewer, egui::Modifiers::ALT);
    assert_eq!(viewer.row_width.bytes(), 16);
    egui::Popup::close_all(&ctx);
    viewer.request(2, &ctx, &runtime.handle());
    let generation = viewer.generation;
    send(&mut viewer, egui::Modifiers::ALT);
    assert_eq!(viewer.row_width.bytes(), 16);
    assert_eq!(viewer.generation, generation);
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 2);
    assert!(!runtime.run_next());
}

#[test]
fn row_width_pointer_preset_and_copy_use_loaded_snapshot_without_work() {
    let temp = TempDir::new();
    let path = temp.path().join("row-width-ui.bin");
    std::fs::write(&path, b"0123456789abcdefXYZ").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.bookmarks[0] = Some(3);
    viewer.search_input = "41".into();
    let mut copied = None;
    for frame in 0..14 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 1000.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 10, 11].contains(&frame) {
            let id = if frame < 10 {
                egui::Id::new(("hex-row-width-preset", 8u8))
            } else {
                egui::Id::new("hex-copy-page")
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 10,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let (_, text) = viewer.show(&ctx, &runtime.handle());
            if text.is_some() {
                copied = text;
            }
        }));
    }
    assert_eq!(viewer.row_width.bytes(), 8);
    assert_eq!(
        copied.as_deref(),
        Some(
            "0000000000000000  30 31 32 33 34 35 36 37  | 01234567\n0000000000000008  38 39 61 62 63 64 65 66  | 89abcdef\n0000000000000010  58 59 5A                 | XYZ"
        )
    );
    assert!(!runtime.run_next());
    assert_eq!(viewer.history.current, Some(0));
    assert_eq!(viewer.bookmarks[0], Some(3));
    assert_eq!(viewer.search_input, "41");
    viewer.reload(&ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.row_width.bytes(), 8);
}

fn remove_recent_entry_via_pointer(search: bool) {
    use crate::hex_search::InputMode;
    let temp = TempDir::new();
    let path = temp.path().join("remove-history.bin");
    std::fs::write(&path, [41; 128]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.recent_offsets = ["0x10".into(), "16".into()].into_iter().collect();
    viewer.recent_searches.record("41", InputMode::Utf8, false);
    viewer.recent_searches.record("41", InputMode::Hex, false);
    viewer.recent_searches.record("41", InputMode::Hex, true);
    viewer.offset_input = "active offset".into();
    viewer.search_input = "active search".into();
    viewer.search_mode = InputMode::Koi8U;
    viewer.bookmarks[0] = Some(7);
    viewer.last_match = Some((0, vec![41]));
    viewer.search_message = Some("retained status".into());
    viewer.error = Some("retained error".into());
    for frame in 0..14 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 1000.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 10, 11].contains(&frame) {
            let id = if frame < 10 {
                egui::Id::new(if search {
                    "hex-recent-searches-menu"
                } else {
                    "hex-recent-offsets-menu"
                })
            } else {
                egui::Id::new((
                    if search {
                        "hex-remove-recent-search"
                    } else {
                        "hex-remove-recent-offset"
                    },
                    0usize,
                ))
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 10,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    }
    assert!(!runtime.run_next());
    assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
    assert_eq!(viewer.page.as_ref().unwrap().bytes, [41; 128]);
    assert_eq!(viewer.history.current, Some(0));
    assert!(viewer.history.past.is_empty() && viewer.history.future.is_empty());
    assert_eq!(viewer.bookmarks[0], Some(7));
    assert_eq!(viewer.offset_input, "active offset");
    assert_eq!(viewer.search_input, "active search");
    assert_eq!(viewer.search_mode, InputMode::Koi8U);
    assert!(!viewer.wrap_search);
    assert_eq!(viewer.last_match, Some((0, vec![41])));
    assert_eq!(viewer.search_message.as_deref(), Some("retained status"));
    assert_eq!(viewer.error.as_deref(), Some("retained error"));
    if search {
        assert_eq!(viewer.recent_searches.0.len(), 2);
        assert!(viewer.recent_searches.0.iter().all(|entry| !entry.wrap));
        assert_eq!(viewer.recent_searches.0[0].mode, InputMode::Hex);
        assert_eq!(viewer.recent_searches.0[1].mode, InputMode::Utf8);
        assert_eq!(viewer.recent_offsets.len(), 2);
    } else {
        assert_eq!(viewer.recent_offsets.len(), 1);
        assert_eq!(viewer.recent_offsets[0], "16");
        assert_eq!(viewer.recent_searches.0.len(), 3);
    }
}

#[test]
fn recent_search_pointer_removes_only_exact_snapshot_without_work_or_recall() {
    remove_recent_entry_via_pointer(true);
}

#[test]
fn recent_offset_pointer_removes_exact_expression_without_work_or_recall() {
    remove_recent_entry_via_pointer(false);
}

#[test]
fn recent_offsets_admit_only_submitted_go_inputs_and_bound_history() {
    let temp = TempDir::new();
    let path = temp.path().join("offsets.bin");
    std::fs::write(&path, [0; 128]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.recent_offsets.is_empty());
    for input in ["bad", "-1", "+129", "101%"] {
        viewer.offset_input = input.into();
        viewer.go_to_input(&ctx, &runtime.handle());
        assert!(!runtime.run_next());
        assert!(viewer.recent_offsets.is_empty());
    }
    // Absolute offsets beyond EOF are admitted by the existing parser;
    // worker rejection still leaves a submitted expression in input history.
    viewer.offset_input = "129".into();
    viewer.go_to_input(&ctx, &runtime.handle());
    assert_eq!(viewer.recent_offsets.front().unwrap(), "129");
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.error.is_some());
    for value in 0..30 {
        viewer.offset_input = value.to_string();
        viewer.go_to_input(&ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
    }
    assert_eq!(viewer.recent_offsets.len(), 20);
    assert_eq!(viewer.recent_offsets.back().unwrap(), "10");
    viewer.offset_input = "20".into();
    viewer.go_to_input(&ctx, &runtime.handle());
    // Busy input cannot queue work or pollute the history.
    viewer.offset_input = "100".into();
    viewer.go_to_input(&ctx, &runtime.handle());
    assert_eq!(viewer.recent_offsets.front().unwrap(), "20");
    assert_eq!(viewer.recent_offsets.len(), 20);
    assert!(runtime.run_next());
    viewer.poll();
    viewer.offset_input = "+1".into();
    viewer.go_to_input(&ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 21);
    assert_eq!(viewer.recent_offsets.front().unwrap(), "+1");
    viewer.reload(&ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.recent_offsets.front().unwrap(), "+1");
}

#[test]
fn recent_offset_pointer_recall_does_not_read_and_relative_go_uses_current_page() {
    let temp = TempDir::new();
    let path = temp.path().join("offset-menu.bin");
    std::fs::write(&path, [0; 128]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(20, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.recent_offsets.push_back("+1".into());
    viewer.search_input = "AA".into();
    viewer.bookmarks[0] = Some(7);
    for frame in 0..14 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 10, 11].contains(&frame) {
            let id = if frame < 10 {
                egui::Id::new("hex-recent-offsets-menu")
            } else {
                egui::Id::new(("hex-recent-offset", 0usize))
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 10,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    }
    assert_eq!(viewer.offset_input, "+1");
    assert!(!runtime.run_next());
    assert_eq!(viewer.page.as_ref().unwrap().offset, 20);
    assert_eq!(viewer.history.current, Some(20));
    assert_eq!(viewer.search_input, "AA");
    assert_eq!(viewer.bookmarks[0], Some(7));
    for frame in 14..17 {
        let rect = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("hex-go")))
            .unwrap();
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        input.events.push(egui::Event::PointerMoved(rect.center()));
        if frame > 14 {
            input.events.push(egui::Event::PointerButton {
                pos: rect.center(),
                button: egui::PointerButton::Primary,
                pressed: frame == 15,
                modifiers: Default::default(),
            });
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    }
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 21);
}

#[test]
fn recent_searches_deduplicate_full_settings_and_bound_count_and_text() {
    use crate::hex_search::InputMode;
    let mut history = SearchHistory::default();
    for index in 0..30 {
        history.record(&format!("{index:02X}"), InputMode::Hex, false);
    }
    assert_eq!(history.0.len(), 20);
    history.record("1D", InputMode::Hex, false);
    assert_eq!(history.0.len(), 20);
    assert_eq!(history.0.front().unwrap().query, "1D");
    history.record("1D", InputMode::Utf8, false);
    history.record("1D", InputMode::Utf8, true);
    assert_ne!(history.0[0].mode, history.0[2].mode);
    assert_eq!(history.0[1].query, "1D");
    assert_eq!(history.0[1].mode, InputMode::Utf8);
    assert!(history.0[0].wrap && !history.0[1].wrap);
    for index in 0..20 {
        history.record(
            &format!("{} {index}", "A".repeat(2000)),
            InputMode::Utf8,
            false,
        );
    }
    assert!(
        history
            .0
            .iter()
            .map(|entry| entry.query.len())
            .sum::<usize>()
            <= 16 * 1024
    );
    let before = history.0.len();
    history.record(&"A".repeat(16 * 1024 + 1), InputMode::Utf8, false);
    assert_eq!(history.0.len(), before);
}

#[test]
fn recent_searches_record_only_valid_submitted_searches_and_recall_without_work() {
    let temp = TempDir::new();
    let path = temp.path().join("recall.bin");
    std::fs::write(&path, b"ABAB").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.recent_searches.0.is_empty());
    viewer.search_input = "bad query".into();
    viewer.search(
        crate::hex_search::Direction::Forward,
        &ctx,
        &runtime.handle(),
    );
    assert!(!runtime.run_next());
    assert!(viewer.recent_searches.0.is_empty());
    viewer.search_input = "41".into();
    viewer.wrap_search = true;
    viewer.search(
        crate::hex_search::Direction::Forward,
        &ctx,
        &runtime.handle(),
    );
    assert_eq!(viewer.recent_searches.0.len(), 1);
    let saved = viewer.recent_searches.0[0].clone();
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_mode = crate::hex_search::InputMode::Utf8;
    viewer.search_input = "other".into();
    viewer.wrap_search = false;
    viewer.recall_search(saved);
    assert_eq!(viewer.search_input, "41");
    assert_eq!(viewer.search_mode, crate::hex_search::InputMode::Hex);
    assert!(viewer.wrap_search);
    assert!(viewer.last_match.is_none());
    assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
    assert!(!runtime.run_next());
    viewer.recent_searches = Default::default();
    assert_eq!(viewer.search_input, "41");
}

#[test]
fn recent_search_menu_pointer_restores_query_encoding_and_wrap_without_searching() {
    let temp = TempDir::new();
    let path = temp.path().join("recent-menu.bin");
    std::fs::write(&path, b"AB").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer
        .recent_searches
        .record("і", crate::hex_search::InputMode::Koi8U, true);
    let mut trace = Vec::new();
    for frame in 0..14 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 10, 11].contains(&frame) {
            let id = if frame < 10 {
                egui::Id::new("hex-recent-searches-menu")
            } else {
                egui::Id::new(("hex-recent-search", 0usize))
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 10,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        trace.push(format!(
            "frame {frame}: menu={:?} item={:?} pointer={:?} query={:?}",
            ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new("hex-recent-searches-menu"))),
            ctx.data(
                |data| data.get_temp::<egui::Rect>(egui::Id::new(("hex-recent-search", 0usize)))
            ),
            ctx.input(|input| input.pointer.latest_pos()),
            viewer.search_input
        ));
    }
    assert_eq!(viewer.search_input, "і", "{}", trace.join("\n"));
    assert_eq!(viewer.search_mode, crate::hex_search::InputMode::Koi8U);
    assert!(viewer.wrap_search);
    assert!(!runtime.run_next());
    assert_eq!(viewer.page.as_ref().unwrap().bytes, b"AB");
}

#[test]
fn legacy_codepage_menu_pointer_action_clears_stale_match_but_retains_page_and_query() {
    let temp = TempDir::new();
    let path = temp.path().join("legacy-menu.bin");
    std::fs::write(&path, b"AB").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "і".into();
    viewer.last_match = Some((0, vec![0x41]));
    viewer.search_message = Some("stale match".into());
    viewer.error = Some("stale input error".into());
    for frame in 0..10 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 6, 7].contains(&frame) {
            let id = if frame < 6 {
                egui::Id::new("hex-legacy-menu")
            } else {
                egui::Id::new(("hex-legacy-mode", "KOI8-U"))
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            if frame == 3 {
                assert!(
                    rect.height() <= 30.0,
                    "Encoding menu must remain a single-line control: {rect:?}"
                );
            }
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 6,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    }
    assert_eq!(viewer.search_mode, crate::hex_search::InputMode::Koi8U);
    assert_eq!(viewer.search_input, "і");
    assert_eq!(viewer.page.as_ref().unwrap().bytes, b"AB");
    assert_eq!(viewer.history.current, Some(0));
    assert!(
        viewer.last_match.is_none() && viewer.search_message.is_none() && viewer.error.is_none()
    );
    assert!(!runtime.run_next());
}

#[test]
fn bookmark_shortcuts_set_and_jump_all_ten_slots_through_guarded_reads() {
    let temp = TempDir::new();
    let path = temp.file("bookmark-keys.bin", "0123456789abcdefghij");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    let keys = [
        egui::Key::Num1,
        egui::Key::Num2,
        egui::Key::Num3,
        egui::Key::Num4,
        egui::Key::Num5,
        egui::Key::Num6,
        egui::Key::Num7,
        egui::Key::Num8,
        egui::Key::Num9,
        egui::Key::Num0,
    ];
    let send = |viewer: &mut HexViewer, key, shift| {
        let modifiers = egui::Modifiers {
            ctrl: true,
            shift,
            ..Default::default()
        };
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    };
    for (slot, key) in keys.into_iter().enumerate() {
        viewer.request(slot as u64, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        send(&mut viewer, key, true);
        assert_eq!(viewer.bookmarks[slot], Some(slot as u64));
        assert_eq!(viewer.bookmark_slot, slot);
        assert!(!runtime.run_next());
    }
    viewer.search_input = "query remains".into();
    for (slot, key) in keys.into_iter().enumerate() {
        send(&mut viewer, key, false);
        assert!(viewer.task.is_some());
        assert!(runtime.run_next());
        viewer.poll();
        let page = viewer.page.as_ref().unwrap();
        assert_eq!(page.offset, slot as u64);
        assert_eq!(page.bytes[0], b'0' + slot as u8);
        assert_eq!(viewer.history.current, Some(slot as u64));
        assert_eq!(viewer.search_input, "query remains");
        assert_eq!(viewer.bookmark_slot, slot);
    }
}

#[test]
fn bookmark_shortcuts_ignore_unrelated_modifiers_busy_errors_and_empty_slots() {
    let temp = TempDir::new();
    let path = temp.file("bookmark-guards.bin", "abcdefghij");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(2, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    let send = |viewer: &mut HexViewer, modifiers: egui::Modifiers| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::Num1,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    };
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::SHIFT,
        egui::Modifiers {
            ctrl: true,
            alt: true,
            ..Default::default()
        },
        egui::Modifiers {
            ctrl: true,
            mac_cmd: true,
            ..Default::default()
        },
    ] {
        send(&mut viewer, modifiers);
        assert_eq!(viewer.bookmarks, [None; 10]);
        assert!(!runtime.run_next());
    }
    send(&mut viewer, egui::Modifiers::CTRL); // Empty slot does not read.
    assert!(!runtime.run_next());
    viewer.error = Some("invalid offset".into());
    send(
        &mut viewer,
        egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
    );
    assert_eq!(viewer.bookmarks, [None; 10]);
    viewer.error = None;
    viewer.bookmarks[0] = Some(7);
    viewer.request(4, &ctx, &runtime.handle());
    let generation = viewer.generation;
    send(&mut viewer, egui::Modifiers::CTRL);
    send(
        &mut viewer,
        egui::Modifiers {
            ctrl: true,
            shift: true,
            ..Default::default()
        },
    );
    assert_eq!(viewer.generation, generation);
    assert_eq!(viewer.bookmarks[0], Some(7));
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4);
    assert!(!runtime.run_next());
    egui::Popup::open_id(&ctx, egui::Id::new("bookmark-guard-popup"));
    send(&mut viewer, egui::Modifiers::CTRL);
    assert!(!runtime.run_next());
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4);
    egui::Popup::close_all(&ctx);
}

#[test]
fn close_keys_cancel_pending_reads_and_do_not_start_initial_reads() {
    for key in [egui::Key::Escape, egui::Key::F10] {
        for pending in [false, true] {
            let temp = TempDir::new();
            let path = temp.file("close.bin", "AB");
            let runtime = crate::workload::DeterministicWorkload::new(Default::default());
            let ctx = egui::Context::default();
            let mut viewer = HexViewer::new(path);
            if pending {
                viewer.request(0, &ctx, &runtime.handle());
            }
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            };
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                let (open, copied) = viewer.show(&ctx, &runtime.handle());
                assert!(!open);
                assert!(copied.is_none());
            }));
            assert_eq!(runtime.stats().cancelled, u64::from(pending));
            assert!(!runtime.run_next());
            viewer.poll();
            assert!(viewer.page.is_none());
            assert!(viewer.task.is_none());
        }
    }
}

#[test]
fn close_keys_respect_popup_and_modifiers() {
    for popup in [false, true] {
        for key in [egui::Key::Escape, egui::Key::F10] {
            let temp = TempDir::new();
            let path = temp.file("close-guard.bin", "AB");
            let runtime = crate::workload::DeterministicWorkload::new(Default::default());
            let ctx = egui::Context::default();
            let mut viewer = HexViewer::new(path);
            viewer.request(0, &ctx, &runtime.handle());
            assert!(runtime.run_next());
            viewer.poll();
            if popup {
                egui::Popup::open_id(&ctx, egui::Id::new("close-popup"));
            }
            let modifiers = if popup {
                egui::Modifiers::NONE
            } else {
                egui::Modifiers::CTRL
            };
            let input = egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            };
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                assert!(viewer.show(&ctx, &runtime.handle()).0);
            }));
            assert!(viewer.page.is_some());
            assert_eq!(runtime.stats().cancelled, 0);
        }
    }
}

#[test]
fn hex_f5_f7_focus_inputs_without_reading_and_preserve_query_and_position() {
    let temp = TempDir::new();
    let path = temp.path().join("focus-commands.bin");
    std::fs::write(&path, b"ABAB").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    for (key, id) in [
        (egui::Key::F5, "hex-offset-input"),
        (egui::Key::F7, "hex-search-query"),
    ] {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 600.0),
            )),
            events: vec![
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new(id))));
        assert!(!runtime.run_next());
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
        assert_eq!(viewer.search_input, "41 42");
    }
}

#[test]
fn keyboard_search_restores_focus_after_disabled_busy_frame_without_stealing_other_focus() {
    let temp = TempDir::new();
    let path = temp.file("restore-focus.bin", "ABxAB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    let frame = |viewer: &mut HexViewer, key: bool| {
        let input = egui::RawInput {
            events: if key {
                vec![egui::Event::Key {
                    key: egui::Key::F3,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }]
            } else {
                vec![]
            },
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            let mut other = String::new();
            ui.add(egui::TextEdit::singleline(&mut other).id(egui::Id::new("other-focus-owner")));
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
    };
    frame(&mut viewer, false);
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("hex-search-query")));
    frame(&mut viewer, true);
    assert!(viewer.task.is_some());
    frame(&mut viewer, false); // Disabled TextEdit drops focus while work is pending.
    assert!(runtime.run_next());
    frame(&mut viewer, false);
    assert!(ctx.memory(|m| m.has_focus(egui::Id::new("hex-search-query"))));
    assert_eq!(viewer.last_match.as_ref().unwrap().0, 0);
    frame(&mut viewer, true); // Repeat without manually requesting focus again.
    assert!(viewer.task.is_some());
    frame(&mut viewer, false);
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("other-focus-owner")));
    assert!(runtime.run_next());
    frame(&mut viewer, false);
    assert_eq!(viewer.last_match.as_ref().unwrap().0, 3);
    assert!(ctx.memory(|m| m.has_focus(egui::Id::new("other-focus-owner"))));
    assert!(!viewer.restore_search_focus);
    viewer.restore_search_focus = true;
    viewer.cancel();
    assert!(!viewer.restore_search_focus);
}

#[test]
fn hex_focus_shortcuts_leave_open_popup_focus_unchanged() {
    let temp = TempDir::new();
    let path = temp.file("focus-popup.bin", "AB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    egui::Popup::open_id(&ctx, egui::Id::new("focus-popup"));
    let input = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::F7,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        ..Default::default()
    };
    crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
        let _ = viewer.show(&ctx, &runtime.handle());
    }));
    assert!(!ctx.memory(|memory| memory.has_focus(egui::Id::new("hex-search-query"))));
    assert!(!runtime.run_next());
}

#[test]
fn hex_focus_shortcuts_ignore_modifiers_and_busy_worker() {
    let temp = TempDir::new();
    let path = temp.path().join("focus-guard.bin");
    std::fs::write(&path, b"AB").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    for (busy, modifiers) in [
        (false, egui::Modifiers::SHIFT),
        (true, egui::Modifiers::NONE),
    ] {
        if busy {
            viewer.request(1, &ctx, &runtime.handle());
        }
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::F7,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        assert!(!ctx.memory(|memory| memory.has_focus(egui::Id::new("hex-search-query"))));
        assert_eq!(runtime.run_next(), busy);
        assert!(!runtime.run_next());
        viewer.poll();
    }
}

#[test]
fn hex_position_history_branches_skips_duplicates_and_bounds_both_stacks() {
    let mut history = PositionHistory::default();
    for offset in 0..100 {
        history.record(offset);
    }
    assert_eq!(history.past.len(), 64);
    history.record(99);
    assert_eq!(history.past.len(), 64);
    let mut count = 0;
    while let Some(offset) = history.target(true) {
        history.commit(true, offset);
        count += 1;
    }
    assert_eq!(count, 64);
    assert_eq!(history.current, Some(35));
    assert_eq!(history.future.len(), 64);
    history.commit(false, 999); // A mismatched completion cannot consume history.
    assert_eq!(history.current, Some(35));
    history.commit(false, 36);
    assert_eq!(history.current, Some(36));
    history.record(500);
    assert_eq!(history.target(false), None);
    assert_eq!(history.target(true), Some(36));
}

#[test]
fn hex_history_commits_only_accepted_pages_and_survives_cancel_failure_until_reload() {
    let temp = TempDir::new();
    let path = temp.path().join("history.bin");
    std::fs::write(&path, vec![0; 9000]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    for offset in [0, 4096] {
        viewer.request(offset, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
    }
    viewer.request_history(true, &ctx, &runtime.handle());
    assert_eq!(viewer.history.current, Some(4096));
    assert_eq!(viewer.history.target(true), Some(0));
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.history.current, Some(0));
    assert_eq!(viewer.history.target(false), Some(4096));
    viewer.request_history(false, &ctx, &runtime.handle());
    viewer.cancel();
    let _ = runtime.run_next();
    viewer.poll();
    assert_eq!(viewer.history.current, Some(0));
    assert_eq!(viewer.history.target(false), Some(4096));
    viewer.request_history(false, &ctx, &runtime.handle());
    std::fs::write(&path, vec![1; 9000]).unwrap();
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.error.is_some());
    assert_eq!(viewer.history.current, Some(0));
    assert_eq!(viewer.history.target(false), Some(4096));
    viewer.reload(&ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.history.current, Some(0));
    assert_eq!(viewer.history.target(true), None);
    assert_eq!(viewer.history.target(false), None);
    assert_eq!(viewer.page.as_ref().unwrap().bytes[0], 1);
}

#[test]
fn hex_history_records_search_hits_and_new_navigation_discards_forward_branch() {
    let temp = TempDir::new();
    let path = temp.path().join("search-history.bin");
    let mut bytes = vec![0; 9000];
    bytes[50] = 0x41;
    bytes[5000] = 0x41;
    std::fs::write(&path, bytes).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41".into();
    for expected in [50, 5000] {
        viewer.search(
            crate::hex_search::Direction::Forward,
            &ctx,
            &runtime.handle(),
        );
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.history.current, Some(expected));
    }
    viewer.request_history(true, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 50);
    assert_eq!(viewer.history.target(false), Some(5000));
    assert_eq!(viewer.search_input, "41");
    viewer.request(100, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.history.target(false), None);
    assert_eq!(viewer.history.target(true), Some(50));
}

#[test]
fn explicit_radix_offsets_support_far_forms_with_strict_conflict_and_overflow_checks() {
    for input in ["0xFF", "$FF", "ffh", "$ffH", "0Xffh", "255m", "255M", "255"] {
        assert_eq!(parse_position(input, None, None), Ok(255), "{input}");
    }
    assert_eq!(parse_position("10h", None, None), Ok(16));
    assert_eq!(parse_position("10m", None, None), Ok(10));
    assert_eq!(
        parse_position("$FFFFFFFFFFFFFFFF", None, None),
        Ok(u64::MAX)
    );
    assert_eq!(
        parse_position("ffffffffffffffffh", None, None),
        Ok(u64::MAX)
    );
    assert_eq!(
        parse_position("18446744073709551615m", None, None),
        Ok(u64::MAX)
    );
    assert_eq!(parse_position("+$10", Some(1000), Some(500)), Ok(516));
    assert_eq!(parse_position("-10h", Some(1000), Some(500)), Ok(484));
    assert_eq!(parse_position("+10m", Some(1000), Some(500)), Ok(510));
    for input in [
        "$",
        "h",
        "m",
        "FF",
        "0x10m",
        "$10M",
        "10mh",
        "10hm",
        "10 h",
        "m10",
        "$0x10",
        "$+FF",
        "0x+FF",
        "FF+h",
        "$10000000000000000",
        "10000000000000000h",
        "18446744073709551616m",
        "10h%",
        "$10%",
        "10m%",
    ] {
        assert!(
            parse_position(input, Some(1000), Some(500)).is_err(),
            "{input}"
        );
    }
}

#[test]
fn explicit_radix_go_values_load_the_same_exact_bytes_and_preserve_relative_guards() {
    let temp = TempDir::new();
    let path = temp.path().join("radix.bin");
    std::fs::write(&path, (0..=255).cycle().take(8192).collect::<Vec<u8>>()).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    for (input, expected) in [
        ("$1001", 4097),
        ("-10h", 4081),
        ("+16m", 4097),
        ("1001H", 4097),
    ] {
        let page = viewer.page.as_ref().unwrap();
        let target =
            parse_position(input, Some(page.stamp.identity.size), Some(page.offset)).unwrap();
        viewer.request(target, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        let page = viewer.page.as_ref().unwrap();
        assert_eq!(page.offset, expected);
        assert_eq!(page.bytes[0], (expected % 256) as u8);
    }
    assert!(parse_position("+$FFFF", Some(8192), Some(4097)).is_err());
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4097);
    assert!(!runtime.run_next());
}

#[test]
fn relative_positions_use_checked_page_base_arithmetic_and_full_file_percentages() {
    for (input, expected) in [
        ("+64", 564),
        ("-0x10", 484),
        (" +10% ", 600),
        ("-10%", 400),
        ("+0", 500),
        ("-0%", 500),
        ("500", 500),
        ("50%", 500),
    ] {
        assert_eq!(
            parse_position(input, Some(1000), Some(500)),
            Ok(expected),
            "{input}"
        );
    }
    assert_eq!(parse_position("+100%", Some(1000), Some(0)), Ok(1000));
    assert_eq!(parse_position("-100%", Some(1000), Some(1000)), Ok(0));
    assert_eq!(
        parse_position("+100%", Some(u64::MAX), Some(0)),
        Ok(u64::MAX)
    );
    for input in [
        "+501", "-501", "+", "-", "++1", "+-1", "--1", "+ 1", "-\t1", "+0x", "-1.5", "+101%",
        "-1%tail",
    ] {
        assert!(
            parse_position(input, Some(1000), Some(500)).is_err(),
            "{input:?}"
        );
    }
    assert!(parse_position("+1", Some(u64::MAX), Some(u64::MAX)).is_err());
    assert!(parse_position("-1", Some(u64::MAX), Some(0)).is_err());
    assert!(parse_position("+1", Some(10), None).is_err());
    assert_eq!(parse_position("+0", Some(0), Some(0)), Ok(0));
}

#[test]
fn relative_positions_drive_exact_worker_pages_and_keep_invalid_input_local() {
    let temp = TempDir::new();
    let path = temp.path().join("relative.bin");
    std::fs::write(&path, (0..=255).cycle().take(9000).collect::<Vec<u8>>()).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(4095, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    for (input, expected) in [
        ("+1", 4096),
        ("-0x10", 4080),
        ("+10%", 4980),
        ("-10%", 4080),
    ] {
        let page = viewer.page.as_ref().unwrap();
        let offset =
            parse_position(input, Some(page.stamp.identity.size), Some(page.offset)).unwrap();
        viewer.request(offset, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        let page = viewer.page.as_ref().unwrap();
        assert_eq!(page.offset, expected);
        assert_eq!(page.bytes[0], (expected % 256) as u8);
    }
    assert!(
        parse_position(
            "-99999",
            Some(9000),
            viewer.page.as_ref().map(|page| page.offset)
        )
        .is_err()
    );
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4080);
    assert!(!runtime.run_next());
    assert_eq!(std::fs::metadata(path).unwrap().len(), 9000);
}

#[test]
fn focused_hex_search_shortcuts_drive_both_directions_and_wrapping() {
    use crate::hex_search::Direction;
    let temp = TempDir::new();
    let path = temp.path().join("shortcuts.bin");
    std::fs::write(&path, b"ABxAB").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    for (frame, key, modifiers, expected) in [
        (1, egui::Key::F3, egui::Modifiers::NONE, 0),
        (2, egui::Key::Enter, egui::Modifiers::NONE, 3),
        (3, egui::Key::F3, egui::Modifiers::SHIFT, 0),
        (4, egui::Key::Enter, egui::Modifiers::SHIFT, 3),
    ] {
        // Register the field before assigning focus, just as a user click would.
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("hex-search-query")));
        if frame == 4 {
            viewer.wrap_search = true;
        }
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        assert!(runtime.run_next(), "shortcut {frame} must queue a search");
        viewer.poll();
        assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
        assert!(!runtime.run_next());
        assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new("hex-search-query"))));
    }
    assert!(
        viewer
            .search_message
            .as_ref()
            .unwrap()
            .starts_with("Wrapped")
    );
    viewer.search(Direction::Forward, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.last_match.as_ref().unwrap().0, 0);
}

#[test]
fn hex_search_shortcuts_respect_focus_and_do_not_duplicate_busy_work() {
    let temp = TempDir::new();
    let path = temp.path().join("focus.bin");
    std::fs::write(&path, b"ABAB").unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    for focused in [false, true] {
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        if focused {
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("hex-search-query")));
            viewer.search(
                crate::hex_search::Direction::Forward,
                &ctx,
                &runtime.handle(),
            );
        } else {
            ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("hex-search-query")));
        }
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::F3,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let _ = viewer.show(&ctx, &runtime.handle());
        }));
        assert_eq!(runtime.run_next(), focused);
        assert!(!runtime.run_next());
        viewer.poll();
    }
}

#[test]
fn copy_match_exports_only_snapshot_match_in_all_formats_without_new_io() {
    for (bytes, label, expected) in [
        (b"AB\n".as_slice(), "HEX bytes", "41 42 0A"),
        (b"AB\n".as_slice(), "Decimal bytes", "065 066 010"),
        (b"AB\n".as_slice(), "UTF-8 text", "AB\n"),
        (b"AB\n".as_slice(), "Clear match", ""),
        ([0xff, 0x00].as_slice(), "UTF-8 text", ""),
    ] {
        let temp = TempDir::new();
        let path = temp.path().join("match.bin");
        let mut data = vec![0x11; 4094];
        data.extend_from_slice(bytes);
        data.push(0x22);
        std::fs::write(&path, data).unwrap();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        viewer.search_input = byte_text(bytes, ByteFormat::Hex);
        viewer.search(
            crate::hex_search::Direction::Forward,
            &ctx,
            &runtime.handle(),
        );
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.matched_bytes(), Some(bytes));
        // Clipboard export belongs to the accepted snapshot, not a new read.
        std::fs::write(&path, b"replacement").unwrap();
        let mut copied = None;
        for frame in 0..10 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 900.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            if [3, 4, 6, 7].contains(&frame) {
                let id = if frame < 6 {
                    egui::Id::new("hex-copy-match-menu")
                } else {
                    egui::Id::new(("hex-copy-match", label))
                };
                let rect = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 3 || frame == 6,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                let (_, text) = viewer.show(&ctx, &runtime.handle());
                if text.is_some() {
                    copied = text;
                }
            }));
        }
        assert_eq!(
            copied.as_deref(),
            if expected.is_empty() {
                None
            } else {
                Some(expected)
            }
        );
        assert!(!runtime.run_next());
        if label == "Clear match" {
            assert!(!viewer.match_selected);
            assert!(viewer.matched_bytes().is_none());
            assert!(viewer.last_match.is_some());
        }
        assert_eq!(viewer.page.as_ref().unwrap().offset, 4094);
        assert_eq!(viewer.history.current, Some(4094));
    }
}

#[test]
fn repeat_search_keys_advance_reverse_and_wrap_without_changing_query_or_bookmark() {
    let temp = TempDir::new();
    let path = temp.file("repeat-keys.bin", "ABxAB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    viewer.wrap_search = true;
    viewer.bookmarks[0] = Some(0);
    viewer.search(
        crate::hex_search::Direction::Forward,
        &ctx,
        &runtime.handle(),
    );
    assert!(runtime.run_next());
    viewer.poll();
    for (key, modifiers, expected) in [
        (egui::Key::F7, egui::Modifiers::SHIFT, 3),
        (egui::Key::F7, egui::Modifiers::ALT, 0),
        (egui::Key::Space, egui::Modifiers::NONE, 3),
        (egui::Key::Space, egui::Modifiers::NONE, 0),
    ] {
        let send = |viewer: &mut HexViewer| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    events: vec![
                        egui::Event::ModifiersChanged(modifiers),
                        egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        },
                    ],
                    ..Default::default()
                },
                |_| {
                    let _ = viewer.show(&ctx, &runtime.handle());
                },
            ))
        };
        send(&mut viewer);
        let generation = viewer.generation;
        send(&mut viewer);
        assert_eq!(viewer.generation, generation);
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
        assert_eq!(viewer.matched_bytes(), Some(b"AB".as_slice()));
        assert_eq!(viewer.history.current, Some(expected));
        assert_eq!(viewer.search_input, "41 42");
        assert_eq!(viewer.bookmarks[0], Some(0));
        assert!(!runtime.run_next());
    }
    assert!(
        viewer
            .search_message
            .as_ref()
            .unwrap()
            .starts_with("Wrapped")
    );
}

#[test]
fn repeat_search_keys_yield_to_fields_popups_and_modified_keys() {
    let temp = TempDir::new();
    let path = temp.file("repeat-guards.bin", "ABxAB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    let send = |viewer: &mut HexViewer, key, modifiers| {
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            },
            |_| {
                let _ = viewer.show(&ctx, &runtime.handle());
            },
        ))
    };
    for id in ["hex-search-query", "hex-offset-input"] {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(id)));
        send(&mut viewer, egui::Key::F7, egui::Modifiers::SHIFT);
        assert!(!runtime.run_next());
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(id)));
    }
    egui::Popup::open_id(&ctx, egui::Id::new("repeat-popup"));
    send(&mut viewer, egui::Key::F7, egui::Modifiers::ALT);
    assert!(!runtime.run_next());
    egui::Popup::close_all(&ctx);
    for (key, modifiers) in [
        (
            egui::Key::F7,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        ),
        (egui::Key::F7, egui::Modifiers::ALT | egui::Modifiers::SHIFT),
        (egui::Key::Space, egui::Modifiers::SHIFT),
        (
            egui::Key::F7,
            egui::Modifiers {
                mac_cmd: true,
                command: true,
                shift: true,
                ..Default::default()
            },
        ),
    ] {
        send(&mut viewer, key, modifiers);
        assert!(!runtime.run_next());
    }
    viewer.search_input = "not hex".into();
    send(&mut viewer, egui::Key::Space, egui::Modifiers::NONE);
    assert!(viewer.error.is_some());
    assert!(!runtime.run_next());
}

#[test]
fn keyboard_match_copy_handles_platform_events_and_preserves_input_owners() {
    let temp = TempDir::new();
    let path = temp.path().join("copy-keys.bin");
    std::fs::write(&path, [0, 0xff, 0x41, 0]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "FF 41".into();
    viewer.search(
        crate::hex_search::Direction::Forward,
        &ctx,
        &runtime.handle(),
    );
    assert!(runtime.run_next());
    viewer.poll();
    viewer.byte_format = ByteFormat::Decimal;
    let send = |viewer: &mut HexViewer, modifiers, mut events: Vec<egui::Event>| {
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let mut copied = None;
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |_| {
                copied = viewer.show(&ctx, &runtime.handle()).1;
            },
        ));
        copied
    };
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let command = egui::Modifiers {
        mac_cmd: true,
        command: true,
        ..Default::default()
    };
    for (modifiers, events) in [
        (
            egui::Modifiers::CTRL,
            vec![key(egui::Key::C, egui::Modifiers::CTRL)],
        ),
        (command, vec![key(egui::Key::C, command)]),
        (
            egui::Modifiers::CTRL,
            vec![key(egui::Key::Insert, egui::Modifiers::CTRL)],
        ),
        (egui::Modifiers::NONE, vec![egui::Event::Copy]),
        (
            egui::Modifiers::CTRL,
            vec![egui::Event::Copy, key(egui::Key::C, egui::Modifiers::CTRL)],
        ),
    ] {
        assert_eq!(
            send(&mut viewer, modifiers, events).as_deref(),
            Some("FF 41")
        );
        assert!(!runtime.run_next());
        assert_eq!(viewer.page.as_ref().unwrap().offset, 1);
    }
    for modifiers in [
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        egui::Modifiers::CTRL | egui::Modifiers::ALT,
        egui::Modifiers::NONE,
    ] {
        assert!(send(&mut viewer, modifiers, vec![key(egui::Key::C, modifiers)]).is_none());
    }
    for id in ["hex-search-query", "hex-offset-input"] {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(id)));
        assert!(send(&mut viewer, egui::Modifiers::NONE, vec![egui::Event::Copy]).is_none());
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(id)));
    }
    egui::Popup::open_id(&ctx, egui::Id::new("copy-key-popup"));
    assert!(send(&mut viewer, egui::Modifiers::NONE, vec![egui::Event::Copy]).is_none());
    egui::Popup::close_all(&ctx);
    viewer.clear_match();
    assert!(send(&mut viewer, egui::Modifiers::NONE, vec![egui::Event::Copy]).is_none());
    viewer.match_selected = true;
    viewer.request(2, &ctx, &runtime.handle());
    let generation = viewer.generation;
    assert!(send(&mut viewer, egui::Modifiers::NONE, vec![egui::Event::Copy]).is_none());
    assert_eq!(viewer.generation, generation);
    viewer.cancel();
    assert!(!runtime.run_next());
}

#[test]
fn keyboard_match_copy_yields_to_real_manual_label_selection() {
    let temp = TempDir::new();
    let path = temp.file("copy-label.bin", "AB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    viewer.search(
        crate::hex_search::Direction::Forward,
        &ctx,
        &runtime.handle(),
    );
    assert!(runtime.run_next());
    viewer.poll();
    let mut rect = egui::Rect::NOTHING;
    for frame in 0..4 {
        let start = egui::pos2(rect.left() + 2.0, rect.center().y);
        let end = egui::pos2(rect.right() - 2.0, rect.center().y);
        let events = match frame {
            1 => vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            2 => vec![egui::Event::PointerMoved(end)],
            3 => vec![
                egui::Event::PointerMoved(end),
                egui::Event::PointerButton {
                    pos: end,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
            _ => Vec::new(),
        };
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                rect = ui
                    .add(egui::Label::new("manual selected text has priority").selectable(true))
                    .rect;
            },
        ));
    }
    assert!(
        ctx.plugin::<egui::text_selection::LabelSelectionState>()
            .lock()
            .has_selection()
    );
    crate::testutil::discard_egui_output(ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        },
        |_| {
            assert!(viewer.show(&ctx, &runtime.handle()).1.is_none());
        },
    ));
    assert!(!runtime.run_next());
}

#[test]
fn clear_match_key_preserves_search_continuation_and_respects_input_owners() {
    let temp = TempDir::new();
    let path = temp.file("clear.bin", "ABxAB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    viewer.bookmarks[0] = Some(0);
    viewer.search(
        crate::hex_search::Direction::Forward,
        &ctx,
        &runtime.handle(),
    );
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.match_selected);
    let send = |viewer: &mut HexViewer, modifiers| {
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key: egui::Key::U,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            },
            |_| {
                let _ = viewer.show(&ctx, &runtime.handle());
            },
        ));
    };
    for modifiers in [
        egui::Modifiers::NONE,
        egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        egui::Modifiers::CTRL | egui::Modifiers::ALT,
        egui::Modifiers {
            ctrl: true,
            mac_cmd: true,
            command: true,
            ..Default::default()
        },
    ] {
        send(&mut viewer, modifiers);
        assert!(viewer.match_selected);
    }
    ctx.memory_mut(|m| m.request_focus(egui::Id::new("hex-offset-input")));
    send(&mut viewer, egui::Modifiers::CTRL);
    assert!(viewer.match_selected);
    ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("hex-offset-input")));
    egui::Popup::open_id(&ctx, egui::Id::new("clear-popup"));
    send(&mut viewer, egui::Modifiers::CTRL);
    assert!(viewer.match_selected);
    egui::Popup::close_all(&ctx);
    send(&mut viewer, egui::Modifiers::CTRL);
    assert!(!viewer.match_selected);
    assert!(viewer.matched_bytes().is_none());
    assert!(viewer.search_message.is_none());
    assert_eq!(viewer.search_input, "41 42");
    assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
    assert_eq!(viewer.bookmarks[0], Some(0));
    assert_eq!(viewer.history.current, Some(0));
    assert!(!runtime.run_next());
    viewer.search(
        crate::hex_search::Direction::Forward,
        &ctx,
        &runtime.handle(),
    );
    let generation = viewer.generation;
    send(&mut viewer, egui::Modifiers::CTRL);
    assert_eq!(viewer.generation, generation);
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(
        viewer.last_match.as_ref().unwrap().0,
        3,
        "clear does not restart from the same match"
    );
    assert_eq!(viewer.matched_bytes(), Some(b"AB".as_slice()));
}

#[test]
fn copy_match_rejects_missing_busy_stale_or_invalid_match_bindings() {
    let temp = TempDir::new();
    let path = temp.file("match-guards.bin", "ABAB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    assert!(viewer.matched_bytes().is_none());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.matched_bytes().is_none());
    viewer.match_selected = true;
    for binding in [
        (1, b"AB".to_vec()),
        (0, b"AC".to_vec()),
        (0, Vec::new()),
        (0, vec![65; 5]),
    ] {
        viewer.last_match = Some(binding);
        assert!(viewer.matched_bytes().is_none());
    }
    viewer.match_selected = true;
    viewer.last_match = Some((0, b"AB".to_vec()));
    assert_eq!(viewer.matched_bytes(), Some(b"AB".as_slice()));
    viewer.error = Some("changed".into());
    assert!(viewer.matched_bytes().is_none());
    viewer.error = None;
    viewer.request(2, &ctx, &runtime.handle());
    assert!(viewer.matched_bytes().is_none());
    viewer.cancel();
    assert!(viewer.matched_bytes().is_none());
    assert!(!runtime.run_next());
}

#[test]
fn plain_byte_copy_menu_pointer_action_returns_hex_snapshot_without_new_work() {
    let temp = TempDir::new();
    let path = temp.path().join("copy.bin");
    std::fs::write(&path, [0, 1, 32, 255]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    let mut copied = None;
    for frame in 0..10 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 900.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 6, 7].contains(&frame) {
            let id = if frame < 6 {
                egui::Id::new("hex-copy-bytes-menu")
            } else {
                egui::Id::new(("hex-copy-bytes", "HEX bytes"))
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(id)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 6,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let (_, text) = viewer.show(&ctx, &runtime.handle());
            if text.is_some() {
                copied = text;
            }
        }));
    }
    assert_eq!(copied.as_deref(), Some("00 01 20 FF"));
    assert!(!runtime.run_next());
    assert_eq!(std::fs::read(path).unwrap(), [0, 1, 32, 255]);
}

#[test]
fn plain_byte_export_round_trips_hex_and_preserves_every_byte_without_offsets() {
    let temp = TempDir::new();
    let path = temp.path().join("bytes");
    let bytes: Vec<_> = (0..=255).cycle().take(PAGE_BYTES).collect();
    std::fs::write(&path, &bytes).unwrap();
    let page = read_page(&path, 0, None, &|| false).unwrap();
    let hex = page.byte_text(ByteFormat::Hex);
    assert_eq!(
        crate::hex_search::parse(&hex[..767]).unwrap(),
        &bytes[..256]
    );
    assert_eq!(hex.split_whitespace().count(), PAGE_BYTES);
    assert_eq!(hex.len(), PAGE_BYTES * 3 - 1);
    assert!(!hex.contains('|'));
    let decoded: Vec<_> = hex
        .split(' ')
        .map(|value| u8::from_str_radix(value, 16).unwrap())
        .collect();
    assert_eq!(decoded, bytes);
    let decimal = page.byte_text(ByteFormat::Decimal);
    assert_eq!(decimal.len(), PAGE_BYTES * 4 - 1);
    let decoded: Vec<_> = decimal
        .split(' ')
        .map(|value| value.parse::<u8>().unwrap())
        .collect();
    assert_eq!(decoded, bytes);
    assert!(std::str::from_utf8(&page.bytes).is_err());
}

#[test]
fn plain_byte_export_uses_loaded_nonzero_page_and_strict_utf8_without_new_io() {
    let temp = TempDir::new();
    let path = temp.path().join("text");
    let text = " ёж🌍\n\t\0 ";
    let mut bytes = vec![0; PAGE_BYTES];
    bytes.extend_from_slice(text.as_bytes());
    std::fs::write(&path, bytes).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(PAGE_BYTES as u64, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    std::fs::remove_file(&path).unwrap();
    let page = viewer.page.as_ref().unwrap();
    assert_eq!(page.offset, PAGE_BYTES as u64);
    assert_eq!(std::str::from_utf8(&page.bytes).unwrap(), text);
    assert_eq!(
        crate::hex_search::parse(&page.byte_text(ByteFormat::Hex)).unwrap(),
        text.as_bytes()
    );
    assert!(!runtime.run_next());
}

#[test]
fn single_byte_codepage_unrepresentable_queries_never_queue_a_search_or_change_page() {
    let temp = TempDir::new();
    let path = temp.path().join("codepages.bin");
    std::fs::write(&path, b"unchanged").unwrap();
    for mode in [
        crate::hex_search::InputMode::Windows1251,
        crate::hex_search::InputMode::Windows1252,
        crate::hex_search::InputMode::Koi8R,
        crate::hex_search::InputMode::Koi8U,
        crate::hex_search::InputMode::Ibm866,
    ] {
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        viewer.search_mode = mode;
        viewer.search_input = "🌍".into();
        viewer.search(
            crate::hex_search::Direction::Forward,
            &ctx,
            &runtime.handle(),
        );
        assert!(!runtime.run_next());
        assert!(
            viewer
                .error
                .as_deref()
                .unwrap()
                .contains("cannot be represented")
        );
        assert!(viewer.task.is_none());
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
        assert_eq!(viewer.page.as_ref().unwrap().bytes, b"unchanged");
    }
}

#[test]
fn cyclic_search_wraps_both_edges_in_every_input_mode_and_keeps_default_off() {
    use crate::hex_search::{CHUNK_BYTES, Direction, InputMode};
    let temp = TempDir::new();
    for mode in [
        InputMode::Hex,
        InputMode::Utf8,
        InputMode::Utf16Le,
        InputMode::Utf16Be,
        InputMode::Windows1251,
        InputMode::Windows1252,
        InputMode::Koi8R,
        InputMode::Koi8U,
        InputMode::Ibm866,
        InputMode::Mixed,
    ] {
        let query = match mode {
            InputMode::Hex => "41 42 41",
            InputMode::Windows1251 => "ёж",
            InputMode::Windows1252 => "café€",
            InputMode::Koi8R => "ёж",
            InputMode::Koi8U => "іґ",
            InputMode::Ibm866 => "Привет",
            InputMode::Mixed => r#""ёж" 00 FF"#,
            _ => "ёж🌍",
        };
        let pattern = mode.parse(query).unwrap();
        let last = CHUNK_BYTES - 1;
        let mut bytes = vec![0; CHUNK_BYTES * 2];
        for offset in [0, last] {
            bytes[offset..offset + pattern.len()].copy_from_slice(&pattern);
        }
        let path = temp.path().join(format!("{mode:?}"));
        std::fs::write(&path, &bytes).unwrap();
        let ctx = egui::Context::default();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let mut viewer = HexViewer::new(path.clone());
        assert!(!viewer.wrap_search);
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        viewer.search_mode = mode;
        viewer.search_input = query.into();
        viewer.search(Direction::Forward, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.last_match.as_ref().unwrap().0, 0);
        viewer.wrap_search = true;
        for (direction, offset) in [(Direction::Backward, last as u64), (Direction::Forward, 0)] {
            viewer.search(direction, &ctx, &runtime.handle());
            assert!(runtime.run_next());
            viewer.poll();
            assert_eq!(viewer.last_match.as_ref().unwrap().0, offset);
            assert!(
                viewer
                    .search_message
                    .as_ref()
                    .unwrap()
                    .starts_with("Wrapped")
            );
        }
        viewer.wrap_search = false;
        viewer.search(Direction::Backward, &ctx, &runtime.handle());
        assert!(viewer.task.is_none());
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
        viewer.wrap_search = true;
        viewer.search_mode = InputMode::Utf8;
        viewer.search_input = "missing".into();
        viewer.search(Direction::Forward, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
        assert_eq!(
            viewer.search_message.as_deref(),
            Some("No match in this file.")
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn file_replacement_between_cyclic_passes_rejects_matches_and_no_match() {
    use crate::hex_search::{CHUNK_BYTES, Direction, Options};
    use std::sync::atomic::Ordering;
    let temp = TempDir::new();
    for direction in [Direction::Forward, Direction::Backward] {
        for matching in [false, true] {
            let path = temp.path().join(format!("{direction:?}-{matching}"));
            let mut data = vec![0; CHUNK_BYTES * 2];
            if matching {
                let offset = if direction == Direction::Forward {
                    0
                } else {
                    data.len() - 1
                };
                data[offset] = 1;
            }
            std::fs::write(&path, &data).unwrap();
            let page = read_page(&path, 0, None, &|| false).unwrap();
            let progress = std::sync::atomic::AtomicU64::new(0);
            let replaced = std::cell::Cell::new(false);
            let start = if direction == Direction::Forward {
                CHUNK_BYTES as u64
            } else {
                CHUNK_BYTES as u64 - 1
            };
            let result = search_page_options(
                &path,
                &page.stamp,
                &[1],
                Options {
                    start: Some(start),
                    direction,
                    wrap: true,
                },
                &|| {
                    if progress.load(Ordering::Relaxed) >= CHUNK_BYTES as u64
                        && !replaced.replace(true)
                    {
                        std::fs::rename(&path, path.with_extension("old")).unwrap();
                        std::fs::write(&path, b"replacement").unwrap();
                    }
                    false
                },
                &progress,
            );
            assert!(result.unwrap_err().contains("changed"));
            assert!(replaced.get());
        }
    }
}

#[test]
fn boundary_search_restarts_at_both_edges_and_next_continues_from_hit() {
    use crate::hex_search::Direction;
    let temp = TempDir::new();
    let path = temp.file("boundary.bin", "ABxxABxxAB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(5, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    for wrap in [false, true] {
        viewer.wrap_search = wrap;
        for (direction, boundary, expected) in [
            (Direction::Forward, true, 0),
            (Direction::Forward, false, 4),
            (Direction::Forward, true, 0),
            (Direction::Backward, true, 8),
            (Direction::Backward, true, 8),
            (Direction::Backward, false, 4),
        ] {
            viewer.search_from(direction, boundary, &ctx, &runtime.handle());
            let generation = viewer.generation;
            // A second command while busy cannot queue another search.
            viewer.search_from(Direction::Forward, true, &ctx, &runtime.handle());
            assert_eq!(viewer.generation, generation);
            assert!(runtime.run_next());
            viewer.poll();
            assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
            assert_eq!(viewer.page.as_ref().unwrap().offset, expected);
            assert!(!runtime.run_next());
        }
    }
    viewer.search_input = "invalid hex".into();
    viewer.search_from(Direction::Forward, true, &ctx, &runtime.handle());
    assert!(viewer.error.is_some());
    assert!(!runtime.run_next());
    assert_eq!(std::fs::read(path).unwrap(), b"ABxxABxxAB");
}

#[test]
fn boundary_search_buttons_pointer_actions_find_first_and_last() {
    let temp = TempDir::new();
    let path = temp.file("buttons.bin", "ABxxAB");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(3, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 42".into();
    let mut frame = 0;
    for (label, expected) in [("Find last", 4), ("Find first", 0)] {
        for phase in 0..5 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                time: Some(frame as f64 / 60.0),
                ..Default::default()
            };
            frame += 1;
            if phase >= 3 {
                let rect = ctx
                    .data(|data| {
                        data.get_temp::<egui::Rect>(egui::Id::new(("hex-boundary-search", label)))
                    })
                    .unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: phase == 3,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
                let _ = viewer.show(&ctx, &runtime.handle());
            }));
        }
        assert!(runtime.run_next(), "{label} must submit a search");
        viewer.poll();
        assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
        assert!(!runtime.run_next());
    }
}

#[test]
fn boundary_search_empty_file_reports_no_match() {
    let temp = TempDir::new();
    let path = temp.file("empty.bin", "");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41".into();
    for direction in [
        crate::hex_search::Direction::Forward,
        crate::hex_search::Direction::Backward,
    ] {
        viewer.search_from(direction, true, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert!(viewer.last_match.is_none());
        assert!(
            viewer
                .search_message
                .as_deref()
                .unwrap()
                .contains("No match")
        );
        assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
    }
}

#[test]
fn byte_search_moves_both_directions_keeps_the_page_on_no_match_and_does_not_wrap() {
    use crate::hex_search::Direction;
    let temp = TempDir::new();
    let path = temp.path().join("search");
    let mut bytes = vec![0x11; 131104];
    for offset in [4094, 65534] {
        bytes[offset..offset + 4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    }
    std::fs::write(&path, &bytes).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "de ad be ef".into();
    for (direction, offset) in [
        (Direction::Forward, 4094),
        (Direction::Forward, 65534),
        (Direction::Forward, 65534),
        (Direction::Backward, 4094),
        (Direction::Backward, 4094),
    ] {
        viewer.search(direction, &ctx, &runtime.handle());
        assert!(viewer.task.as_ref().unwrap().progress.is_some());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.page.as_ref().unwrap().offset, offset);
        assert_eq!(viewer.last_match.as_ref().unwrap().0, offset);
        assert_eq!(
            &viewer.page.as_ref().unwrap().bytes[..4],
            &[0xde, 0xad, 0xbe, 0xef]
        );
    }
    assert!(
        viewer
            .search_message
            .as_deref()
            .unwrap()
            .contains("No match")
    );
    assert_eq!(runtime.stats().cancelled, 0);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn byte_search_handles_overlap_and_its_last_completion_cannot_survive_cancel() {
    use crate::hex_search::Direction;
    let temp = TempDir::new();
    let path = temp.file("overlap", "AAAAA");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_input = "41 41 41".into();
    for expected in [0, 1, 2] {
        viewer.search(Direction::Forward, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
    }
    viewer.search(Direction::Backward, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.task = None; // discard a completed search before its response is consumed
    viewer.request(4, &ctx, &runtime.handle());
    viewer.poll();
    assert!(viewer.page.is_none());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4);
    assert!(viewer.last_match.is_none());
}

#[test]
fn file_replacement_during_search_does_not_publish_match_or_no_match() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let temp = TempDir::new();
    for matching in [false, true] {
        let path = temp.path().join(format!("changing-{matching}"));
        let mut data = vec![0; crate::hex_search::CHUNK_BYTES * 2];
        if matching {
            data[crate::hex_search::CHUNK_BYTES + 2] = 1;
        }
        std::fs::write(&path, data).unwrap();
        let stamp = read_page(&path, 0, None, &|| false).unwrap().stamp;
        let progress = AtomicU64::new(0);
        let replaced = std::cell::Cell::new(false);
        let result = search_page(
            &path,
            &stamp,
            0,
            &[1],
            crate::hex_search::Direction::Forward,
            &|| {
                if progress.load(Ordering::Relaxed) >= crate::hex_search::CHUNK_BYTES as u64
                    && !replaced.replace(true)
                {
                    std::fs::rename(&path, temp.path().join(format!("old-{matching}"))).unwrap();
                    std::fs::write(&path, "new file").unwrap();
                }
                false
            },
            &progress,
        );
        assert!(result.unwrap_err().contains("changed"));
    }
}

#[test]
fn viewer_has_one_pending_read_and_closing_cancels_only_its_task() {
    let temp = TempDir::new();
    let path = temp.file("pages", &"a".repeat(PAGE_BYTES * 2));
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let handle = runtime.handle();
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &handle);
    viewer.request(4096, &ctx, &handle);
    assert_eq!(runtime.stats().queued, 1);
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
    assert_eq!(runtime.stats().cancelled, 0);
    viewer.request(4096, &ctx, &handle);
    assert!(viewer.page.is_none());
    drop(viewer);
    assert_eq!(runtime.stats().cancelled, 1);
    assert!(!runtime.run_next());
    let mut next = HexViewer::new(path);
    next.request(4096, &ctx, &handle);
    assert!(runtime.run_next());
    next.poll();
    assert_eq!(next.page.as_ref().unwrap().offset, 4096);
    assert_eq!(runtime.stats().cancelled, 1);
}

#[test]
fn cancelled_completion_cannot_replace_a_newer_page_and_abandonment_is_terminal() {
    let temp = TempDir::new();
    let path = temp.file("pages", &"b".repeat(PAGE_BYTES * 2));
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let handle = runtime.handle();
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &ctx, &handle);
    assert!(runtime.run_next()); // completion is queued in the old receiver
    viewer.task = None; // Cancel/close discards that receiver
    viewer.request(4096, &ctx, &handle);
    viewer.poll();
    assert!(viewer.page.is_none() && viewer.error.is_none());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4096);
    viewer.request(0, &ctx, &handle);
    assert!(viewer.task.as_ref().unwrap().handle.cancel());
    viewer.poll();
    assert!(viewer.task.is_none() && viewer.page.is_none());
    assert!(viewer.error.as_deref().unwrap().contains("stopped"));
}

#[test]
fn admission_failure_does_not_leave_a_spinner_or_a_stale_page() {
    let temp = TempDir::new();
    let path = temp.file("file", "abc");
    let runtime = crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits {
        max_queued: 0,
        ..Default::default()
    });
    let mut viewer = HexViewer::new(path);
    viewer.request(0, &egui::Context::default(), &runtime.handle());
    assert!(viewer.task.is_none() && viewer.page.is_none());
    assert!(viewer.error.as_deref().unwrap().contains("queue is full"));
}

#[test]
fn pages_preserve_all_bytes_and_absolute_offsets() {
    let temp = TempDir::new();
    let path = temp.path().join("binary");
    let bytes: Vec<u8> = (0..8197).map(|index| index as u8).collect();
    std::fs::write(&path, &bytes).unwrap();
    let first = read_page(&path, 0, None, &|| false).unwrap();
    assert_eq!(first.bytes, bytes[..PAGE_BYTES]);
    assert_eq!(first.next_offset(), Some(4096));
    let next = read_page(&path, 4096, Some(&first.stamp), &|| false).unwrap();
    assert_eq!(next.bytes, bytes[4096..8192]);
    assert!(next.row(0).starts_with("0000000000001000  00 01 02"));
    let last = read_page(&path, 8192, Some(&first.stamp), &|| false).unwrap();
    assert_eq!(last.bytes, [0, 1, 2, 3, 4]);
    assert_eq!(last.next_offset(), None);
    assert!(last.text().starts_with("0000000000002000  00 01 02 03 04"));
    assert!(last.text().ends_with(" | ....."));
    let unaligned = read_page(&path, 31, Some(&first.stamp), &|| false).unwrap();
    assert_eq!(unaligned.bytes, bytes[31..31 + PAGE_BYTES]);
    assert!(unaligned.row(1).starts_with("000000000000002F"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn decimal_mode_pointer_switch_and_copy_use_loaded_page_without_new_io() {
    let temp = TempDir::new();
    let path = temp.path().join("bytes");
    std::fs::write(&path, [0, 1, 32, 127, 128, 255]).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.byte_format, ByteFormat::Hex);
    let mut copied = None;
    for frame in 0..9 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 760.0),
            )),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        };
        if [3, 4, 6, 7].contains(&frame) {
            let key = if frame < 6 {
                egui::Id::new(("hex-byte-format", "Decimal"))
            } else {
                egui::Id::new("hex-copy-page")
            };
            let rect = ctx.data(|data| data.get_temp::<egui::Rect>(key)).unwrap();
            input.events = vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed: frame == 3 || frame == 6,
                    modifiers: Default::default(),
                },
            ];
        }
        crate::testutil::discard_egui_output(ctx.run_ui(input, |_| {
            let (_, text) = viewer.show(&ctx, &runtime.handle());
            if text.is_some() {
                copied = text;
            }
        }));
    }
    assert_eq!(viewer.byte_format, ByteFormat::Decimal);
    assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
    assert_eq!(
        copied.as_deref(),
        Some(
            viewer
                .page
                .as_ref()
                .unwrap()
                .text_in_format(ByteFormat::Decimal)
                .as_str()
        )
    );
    assert!(copied.unwrap().contains("000 001 032 127 128 255"));
    assert!(
        !runtime.run_next(),
        "changing representation must not reread the file"
    );
    assert_eq!(std::fs::read(path).unwrap(), [0, 1, 32, 127, 128, 255]);
    assert_eq!(
        HexViewer::new(temp.path().join("other")).byte_format,
        ByteFormat::Hex
    );
}

#[test]
fn decimal_bytes_round_trip_all_values_keep_ascii_and_align_short_rows() {
    let temp = TempDir::new();
    let path = temp.path().join("bytes");
    std::fs::write(&path, (0..=255).collect::<Vec<u8>>()).unwrap();
    let page = read_page(&path, 0, None, &|| false).unwrap();
    for row in 0..16 {
        let decimal = page.row_in_format(row, ByteFormat::Decimal);
        let (values, ascii) = decimal[18..].split_once(" | ").unwrap();
        assert_eq!(
            values
                .split_whitespace()
                .map(|value| value.parse::<u8>().unwrap())
                .collect::<Vec<_>>(),
            page.bytes[row * 16..row * 16 + 16]
        );
        assert_eq!(ascii, page.row(row).split_once(" | ").unwrap().1);
    }
    let tail = read_page(&path, 253, Some(&page.stamp), &|| false).unwrap();
    let text = tail.text_in_format(ByteFormat::Decimal);
    assert!(text.starts_with("00000000000000FD  253 254 255 "));
    assert!(text.ends_with(" | ..."));
    assert_eq!(
        text.find(" | "),
        page.row_in_format(0, ByteFormat::Decimal).find(" | ")
    );
    assert_eq!(std::fs::read(path).unwrap(), page.bytes);
}

#[test]
fn ascii_column_never_emits_controls_and_hex_is_exact() {
    let temp = TempDir::new();
    let path = temp.path().join("all-bytes");
    std::fs::write(&path, (0..=255).collect::<Vec<u8>>()).unwrap();
    let page = read_page(&path, 0, None, &|| false).unwrap();
    for row in 0..16 {
        let text = page.row(row);
        let (hex, ascii) = text[18..].split_once(" | ").unwrap();
        let reconstructed: Vec<u8> = hex
            .split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).unwrap())
            .collect();
        assert_eq!(reconstructed, page.bytes[row * 16..row * 16 + 16]);
        assert_eq!(ascii.len(), 16);
        assert!(ascii.bytes().all(|byte| (0x20..=0x7e).contains(&byte)));
    }
    assert!(page.row(2).ends_with(" |  !\"#$%&'()*+,-./"));
}

#[test]
fn empty_files_eof_and_out_of_range_are_distinct() {
    let temp = TempDir::new();
    let path = temp.file("empty", "");
    let empty = read_page(&path, 0, None, &|| false).unwrap();
    assert!(empty.bytes.is_empty() && empty.text().is_empty());
    assert_eq!(empty.next_offset(), None);
    assert!(
        read_page(&path, 1, None, &|| false)
            .unwrap_err()
            .contains("beyond")
    );
    let path = temp.file("three", "abc");
    let eof = read_page(&path, 3, None, &|| false).unwrap();
    assert!(eof.bytes.is_empty());
    assert_eq!(eof.stamp.identity.size, 3);
}

#[test]
fn offsets_are_strict_and_checked_without_integer_wrap() {
    assert_eq!(parse_offset(" 0x2000 ", None), Ok(8192));
    assert_eq!(parse_offset("0XfF", None), Ok(255));
    assert_eq!(parse_offset("4096", None), Ok(4096));
    assert_eq!(parse_offset("18446744073709551615", None), Ok(u64::MAX));
    for value in [
        "",
        "-1",
        "0x",
        "1.5",
        "12tail",
        "0x10000000000000000",
        "18446744073709551616",
    ] {
        assert!(parse_offset(value, None).is_err(), "{value}");
    }
}

#[test]
fn utf16_search_finds_odd_byte_positions_and_split_code_units_in_both_directions() {
    use crate::hex_search::{CHUNK_BYTES, Direction, InputMode};
    let temp = TempDir::new();
    for mode in [InputMode::Utf16Le, InputMode::Utf16Be] {
        let path = temp.path().join(format!("{mode:?}"));
        let pattern = mode.parse(" я🌍 ").unwrap();
        let mut data = vec![0xff; CHUNK_BYTES * 2];
        for offset in [4095, CHUNK_BYTES - 1] {
            data[offset..offset + pattern.len()].copy_from_slice(&pattern);
        }
        std::fs::write(&path, &data).unwrap();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let ctx = egui::Context::default();
        let mut viewer = HexViewer::new(path.clone());
        viewer.request(0, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        viewer.search_mode = mode;
        viewer.search_input = " я🌍 ".into();
        for (direction, expected) in [
            (Direction::Forward, 4095),
            (Direction::Forward, (CHUNK_BYTES - 1) as u64),
            (Direction::Backward, 4095),
        ] {
            viewer.search(direction, &ctx, &runtime.handle());
            assert!(runtime.run_next());
            viewer.poll();
            assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
            assert_eq!(
                &viewer.page.as_ref().unwrap().bytes[..pattern.len()],
                &pattern
            );
        }
        assert_eq!(std::fs::read(path).unwrap(), data);
    }
}

#[test]
fn utf8_search_streams_across_boundaries_in_both_directions_without_decoding_the_file() {
    use crate::hex_search::{CHUNK_BYTES, Direction, InputMode};
    let temp = TempDir::new();
    let path = temp.path().join("utf8-in-binary");
    let text = " Привет🌍 ";
    let mut data = vec![0xff; CHUNK_BYTES * 2];
    for offset in [4094, CHUNK_BYTES - 2] {
        data[offset..offset + text.len()].copy_from_slice(text.as_bytes());
    }
    std::fs::write(&path, &data).unwrap();
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.search_mode = InputMode::Utf8;
    viewer.search_input = text.into();
    for (direction, expected) in [
        (Direction::Forward, 4094),
        (Direction::Forward, (CHUNK_BYTES - 2) as u64),
        (Direction::Backward, 4094),
    ] {
        viewer.search(direction, &ctx, &runtime.handle());
        assert!(runtime.run_next());
        viewer.poll();
        assert_eq!(viewer.last_match.as_ref().unwrap().0, expected);
        assert_eq!(
            &viewer.page.as_ref().unwrap().bytes[..text.len()],
            text.as_bytes()
        );
    }
    viewer.search_input = " привет🌍 ".into();
    viewer.last_match = None;
    viewer.search(Direction::Forward, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.search_message.as_ref().unwrap().contains("No match"));
    assert_eq!(viewer.page.as_ref().unwrap().offset, 4094);
    assert_eq!(std::fs::read(&path).unwrap(), data);
}

#[test]
fn bookmark_slots_keep_exact_offsets_overwrite_independently_and_are_viewer_local() {
    let temp = TempDir::new();
    let path = temp.file("bytes", "0123456789abcdef");
    let mut viewer = HexViewer::new(path.clone());
    viewer.set_bookmark();
    assert_eq!(viewer.bookmarks, [None; 10]);
    for slot in 0..10 {
        viewer.bookmark_slot = slot;
        viewer.accept_page(read_page(&path, slot as u64, None, &|| false).unwrap());
        viewer.set_bookmark();
    }
    assert_eq!(
        viewer.bookmarks,
        std::array::from_fn(|slot| Some(slot as u64))
    );
    viewer.bookmark_slot = 4;
    viewer.accept_page(read_page(&path, 16, None, &|| false).unwrap());
    viewer.set_bookmark();
    assert_eq!(viewer.bookmarks[4], Some(16));
    assert_eq!(viewer.bookmarks[3], Some(3));
    assert_eq!(HexViewer::new(path).bookmarks, [None; 10]);
}

#[test]
fn bookmark_navigation_keeps_stamp_guard_and_reload_clears_all_slots() {
    let temp = TempDir::new();
    let path = temp.file("bytes", "0123456789abcdef");
    let runtime = crate::workload::DeterministicWorkload::new(Default::default());
    let ctx = egui::Context::default();
    let mut viewer = HexViewer::new(path.clone());
    viewer.request(9, &ctx, &runtime.handle());
    viewer.set_bookmark();
    assert_eq!(
        viewer.bookmarks, [None; 10],
        "in-flight pages cannot be bookmarked"
    );
    assert!(runtime.run_next());
    viewer.poll();
    viewer.set_bookmark();
    assert_eq!(viewer.bookmarks[0], Some(9));
    viewer.request(0, &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    viewer.request(viewer.bookmarks[0].unwrap(), &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().offset, 9);
    assert_eq!(viewer.page.as_ref().unwrap().bytes, b"9abcdef");
    std::fs::write(&path, b"changed").unwrap();
    viewer.request(viewer.bookmarks[0].unwrap(), &ctx, &runtime.handle());
    assert!(runtime.run_next());
    viewer.poll();
    assert!(viewer.error.is_some());
    assert!(viewer.page.is_none());
    viewer.bookmark_slot = 8;
    viewer.set_bookmark();
    assert_eq!(
        viewer.bookmarks[8], None,
        "failed reads cannot be bookmarked"
    );
    viewer.reload(&ctx, &runtime.handle());
    assert_eq!(viewer.bookmarks, [None; 10]);
    assert!(runtime.run_next());
    viewer.poll();
    assert_eq!(viewer.page.as_ref().unwrap().bytes, b"changed");
    assert_eq!(viewer.page.as_ref().unwrap().offset, 0);
}

#[test]
fn percentage_offsets_are_exact_bounded_and_require_a_known_size() {
    for size in [0, 1, 8197, u64::MAX] {
        for percent in 0..=100_u64 {
            let expected = size / 100 * percent + size % 100 * percent / 100;
            assert_eq!(
                parse_offset(&format!("{percent}%"), Some(size)),
                Ok(expected)
            );
        }
    }
    assert_eq!(parse_offset(" 50% ", Some(8197)), Ok(4098));
    assert!(parse_offset("50%", None).is_err());
    for input in [
        "%", "101%", "256%", "-1%", "+1%", "1.5%", "50 %", "0x32%", "50%%", "５０%",
    ] {
        assert!(parse_offset(input, Some(8197)).is_err(), "{input}");
    }
}

#[test]
fn percentage_navigation_reads_exact_bytes_and_validates_the_file_stamp() {
    let temp = TempDir::new();
    let path = temp.path().join("percentage.bin");
    let bytes = (0..8197).map(|value| value as u8).collect::<Vec<_>>();
    std::fs::write(&path, &bytes).unwrap();
    let initial = read_page(&path, 0, None, &|| false).unwrap();
    for percent in [0, 1, 50, 99, 100] {
        let offset =
            parse_offset(&format!("{percent}%"), Some(initial.stamp.identity.size)).unwrap();
        let page = read_page(&path, offset, Some(&initial.stamp), &|| false).unwrap();
        assert_eq!(page.offset, offset);
        assert_eq!(
            page.bytes,
            bytes[offset as usize..bytes.len().min(offset as usize + PAGE_BYTES)]
        );
    }
    std::fs::write(&path, b"changed size").unwrap();
    let offset = parse_offset("50%", Some(initial.stamp.identity.size)).unwrap();
    assert!(read_page(&path, offset, Some(&initial.stamp), &|| false).is_err());
}

#[test]
fn sparse_files_beyond_four_gib_use_u64_seeking_and_bounded_memory() {
    use std::io::Write;
    let temp = TempDir::new();
    let path = temp.path().join("sparse");
    let mut file = std::fs::File::create(&path).unwrap();
    let offset = (1u64 << 32) + 37;
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(b"tail").unwrap();
    drop(file);
    let page = read_page(&path, offset, None, &|| false).unwrap();
    assert_eq!(page.bytes, b"tail");
    assert_eq!(page.stamp.identity.size, offset + 4);
    assert!(page.text().starts_with("0000000100000025"));
    for direction in [
        crate::hex_search::Direction::Forward,
        crate::hex_search::Direction::Backward,
    ] {
        let found = search_page(
            &path,
            &page.stamp,
            offset,
            b"tail",
            direction,
            &|| false,
            &std::sync::atomic::AtomicU64::new(0),
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.offset, offset);
    }
}

#[test]
fn cancellation_and_replacement_never_publish_a_partial_page() {
    let temp = TempDir::new();
    let path = temp.file("input", &"a".repeat(PAGE_BYTES * 2));
    let first = read_page(&path, 0, None, &|| false).unwrap();
    let ticks = std::cell::Cell::new(0);
    assert!(
        read_page(&path, 0, None, &|| {
            ticks.set(ticks.get() + 1);
            ticks.get() == 3
        })
        .unwrap_err()
        .contains("cancelled")
    );
    let ticks = std::cell::Cell::new(0);
    assert!(
        read_page(&path, 0, None, &|| {
            ticks.set(ticks.get() + 1);
            if ticks.get() == 4 {
                std::fs::rename(&path, temp.path().join("old")).unwrap();
                std::fs::write(&path, "replacement").unwrap();
            }
            false
        })
        .unwrap_err()
        .contains("changed")
    );
    assert!(
        read_page(&path, 0, Some(&first.stamp), &|| false)
            .unwrap_err()
            .contains("changed")
    );
}

#[cfg(unix)]
#[test]
fn symlinks_directories_and_fifo_are_not_opened_as_files() {
    let temp = TempDir::new();
    let path = temp.file("input", "abc");
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(
        read_page(&link, 0, None, &|| false)
            .unwrap_err()
            .contains("regular")
    );
    assert!(
        read_page(temp.path(), 0, None, &|| false)
            .unwrap_err()
            .contains("regular")
    );
    let fifo = temp.path().join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(
        read_page(&fifo, 0, None, &|| false)
            .unwrap_err()
            .contains("regular")
    );
    let changed = std::cell::Cell::new(0);
    assert!(
        read_page(&path, 0, None, &|| {
            changed.set(changed.get() + 1);
            if changed.get() == 2 {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink(&link, &path).unwrap();
            }
            false
        })
        .is_err()
    );
}
