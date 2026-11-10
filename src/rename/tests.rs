use super::*;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn existing(list: &[&str]) -> HashSet<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Plan a batch whose targets are all regular files.
fn plan_files(names: &[String], existing: &HashSet<String>, rule: &RenameRule) -> Vec<RenamePlan> {
    plan_batch_rename(names, existing, rule, &HashSet::new())
}

#[test]
fn name_list_text_is_lossless_bounded_and_rejects_bom_or_linebreak_ambiguity() {
    let names = vec![
        " leading ".into(),
        "ёж\".txt".into(),
        "\u{feff}later".into(),
    ];
    let text = name_list_text(&names).unwrap();
    assert_eq!(
        parse_name_list(&text).unwrap(),
        names.iter().map(String::as_str).collect::<Vec<_>>()
    );
    for bad in [
        vec![],
        vec![String::new()],
        vec!["first\nsecond".into()],
        vec!["a\r".into()],
        vec!["\u{feff}first".into()],
        vec!["x".repeat(MAX_NAME_LIST_BYTES + 1)],
        vec!["x".into(); 10001],
    ] {
        assert!(name_list_text(&bad).is_err());
    }
    assert_eq!(
        name_list_text(&["x".repeat(MAX_NAME_LIST_BYTES)])
            .unwrap()
            .len(),
        MAX_NAME_LIST_BYTES
    );
}

#[test]
fn ordered_name_list_parsing_and_final_mapping_preserve_original_sources() {
    assert_eq!(
        parse_name_list("\u{feff} first \r\nsecond\n").unwrap(),
        [" first ", "second"]
    );
    assert_eq!(parse_name_list("last\r").unwrap(), ["last\r"]);
    for invalid in ["", "\n", "a\n\n"] {
        assert!(parse_name_list(invalid).is_err());
    }
    assert!(parse_name_list(&"x\n".repeat(10000)).is_ok());
    assert!(parse_name_list(&"x\n".repeat(10001)).is_err());
    assert!(parse_name_list(&"x".repeat(MAX_NAME_LIST_BYTES + 1)).is_err());
    let original = names(&["a.txt", "b.bin"]);
    let mut rule = RenameRule {
        name_list: Some("ALPHA.CSV\nBETA.TSV\n".into()),
        prefix: "P-".into(),
        case: CaseMode::Lower,
        extension_case: ExtensionCase::Lower,
        ..Default::default()
    };
    let plans = plan_files(&original, &HashSet::new(), &rule);
    assert_eq!(
        (&*plans[0].from, &*plans[0].to, plans[0].status),
        ("a.txt", "P-alpha.csv", PlanStatus::Ok)
    );
    assert_eq!((&*plans[1].from, &*plans[1].to), ("b.bin", "P-beta.tsv"));
    for invalid in ["one.txt", "one.txt\ntwo.txt\nthree.txt", "one.txt\n\n"] {
        rule.name_list = Some(invalid.into());
        assert!(name_list_error(&rule, 2).is_some());
        assert!(
            plan_files(&original, &HashSet::new(), &rule)
                .iter()
                .all(|p| p.status == PlanStatus::Invalid)
        );
    }
    rule = RenameRule {
        name_list: Some("same.txt\nsame.txt".into()),
        ..Default::default()
    };
    assert!(
        plan_files(&original, &HashSet::new(), &rule)
            .iter()
            .all(|p| p.status == PlanStatus::Collision)
    );
    rule.name_list = Some("b.bin\na.txt".into());
    assert!(
        plan_files(&original, &HashSet::new(), &rule)
            .iter()
            .all(|p| p.status == PlanStatus::Ok)
    );
    assert_eq!(
        serde_json::from_str::<RenameRule>(&serde_json::to_string(&rule).unwrap()).unwrap(),
        rule
    );
    assert!(
        serde_json::from_str::<RenameRule>("{}")
            .unwrap()
            .name_list
            .is_none()
    );
    assert!(
        serde_json::to_value(RenameRule::default())
            .unwrap()
            .get("name_list")
            .is_none()
    );
    let modified = HashMap::from([
        ("a.txt".into(), std::time::UNIX_EPOCH),
        (
            "b.bin".into(),
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(946684800),
        ),
    ]);
    rule = RenameRule {
        name_list: Some("one.txt\ntwo.txt".into()),
        date_stamp: Some(DateStamp {
            format: "%Y_".into(),
            position: DatePosition::Prefix,
        }),
        ..Default::default()
    };
    let plans = plan_batch_rename_in_folder(
        &original,
        &HashSet::new(),
        &rule,
        &HashSet::new(),
        crate::rename_order::NameFolding::CaseFolded,
        &modified,
        None,
    );
    assert_eq!(
        plans.iter().map(|p| p.to.as_str()).collect::<Vec<_>>(),
        ["1970_one.txt", "2000_two.txt"]
    );
}

#[test]
fn invalid_character_replacement_is_opt_in_bounded_and_collision_checked() {
    let names = vec!["a.txt".to_owned()];
    let mut rule = RenameRule {
        prefix: "dir/\0".into(),
        suffix: "?".into(),
        ..Default::default()
    };
    assert_eq!(
        plan_files(&names, &HashSet::new(), &rule)[0].status,
        PlanStatus::Invalid
    );
    rule.invalid_char_replacement = Some("_".into());
    let plan = plan_files(&names, &HashSet::new(), &rule);
    assert_eq!(
        (&*plan[0].to, plan[0].status),
        ("dir__a?.txt", PlanStatus::Ok)
    );
    rule.invalid_char_replacement = Some(String::new());
    assert_eq!(
        plan_files(&names, &HashSet::new(), &rule)[0].to,
        "dira?.txt"
    );
    assert_eq!(
        serde_json::from_str::<RenameRule>(&serde_json::to_string(&rule).unwrap()).unwrap(),
        rule
    );
    assert!(
        serde_json::from_str::<RenameRule>("{}")
            .unwrap()
            .invalid_char_replacement
            .is_none()
    );
    assert!(
        serde_json::to_value(RenameRule::default())
            .unwrap()
            .get("invalid_char_replacement")
            .is_none()
    );
    for replacement in ["/".into(), "\0".into(), "x".repeat(33)] {
        rule.invalid_char_replacement = Some(replacement);
        assert!(regex_error(&rule).is_some());
        assert_eq!(
            plan_files(&names, &HashSet::new(), &rule)[0].status,
            PlanStatus::Invalid
        );
    }
    rule = RenameRule {
        find: "[- ]".into(),
        replace: "/".into(),
        regex: true,
        invalid_char_replacement: Some("_".into()),
        ..Default::default()
    };
    let collision = plan_files(
        &["a-b.txt".into(), "a b.txt".into()],
        &HashSet::new(),
        &rule,
    );
    assert!(
        collision
            .iter()
            .all(|p| p.to == "a_b.txt" && p.status == PlanStatus::Collision)
    );
    assert_eq!(
        plan_files(
            &["a-b.txt".into()],
            &HashSet::from(["a_b.txt".into()]),
            &rule
        )[0]
        .status,
        PlanStatus::Collision
    );
    rule = RenameRule {
        find: "a".into(),
        invalid_char_replacement: Some(String::new()),
        ..Default::default()
    };
    for replacement in ["/", "./", "../"] {
        rule.replace = replacement.into();
        assert_eq!(
            plan_files(&["a".into()], &HashSet::new(), &rule)[0].status,
            PlanStatus::Invalid
        );
    }
    rule = RenameRule {
        prefix: "/".repeat(3000),
        invalid_char_replacement: Some("_".repeat(32)),
        ..Default::default()
    };
    assert_eq!(
        plan_files(&names, &HashSet::new(), &rule)[0].status,
        PlanStatus::ExpansionLimit
    );
}

#[test]
fn literal_replacement_preserves_dollars_in_regex_and_sequential_pairs() {
    let mut rule = RenameRule {
        find: "(?P<word>old)".into(),
        replace: "$1-${word}-$$".into(),
        regex: true,
        literal_replacement: true,
        replace_target: ReplaceTarget::Stem,
        ..Default::default()
    };
    for (first_only, expected) in [
        (false, "$1-${word}-$$-$1-${word}-$$.txt"),
        (true, "$1-${word}-$$-old.txt"),
    ] {
        rule.first_only = first_only;
        let compiled = compile_find(&rule).unwrap();
        assert_eq!(
            rename_one("old-old.txt", false, 0, &rule, Some(&compiled)),
            expected
        );
    }
    rule.extra_replacements.push(ReplacementPair {
        find: "\\$1".into(),
        replace: "${other}".into(),
    });
    let compiled = compile_find(&rule).unwrap();
    assert_eq!(
        rename_one("old.txt", false, 0, &rule, Some(&compiled)),
        "${other}-${word}-$$.txt"
    );
    let encoded = serde_json::to_string(&rule).unwrap();
    assert_eq!(serde_json::from_str::<RenameRule>(&encoded).unwrap(), rule);
    let old: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(!old.literal_replacement);
    assert!(
        serde_json::to_value(old)
            .unwrap()
            .get("literal_replacement")
            .is_none()
    );
    rule.extra_replacements.clear();
    rule.literal_replacement = false;
    let compiled = compile_find(&rule).unwrap();
    assert_eq!(
        rename_one("old.txt", false, 0, &rule, Some(&compiled)),
        "old-old-$.txt"
    );
    // Sequential replacement still refuses outputs larger than its existing bound.
    rule.literal_replacement = true;
    rule.replace = "x".repeat(MAX_REPLACEMENT_OUTPUT_BYTES + 1);
    rule.extra_replacements.push(ReplacementPair {
        find: "x".into(),
        replace: "y".into(),
    });
    let compiled = compile_find(&rule).unwrap();
    assert!(rename_one_checked("old.txt", false, 0, &rule, Some(&compiled)).is_err());
}

#[test]
fn parent_folder_affixes_compose_with_dates_and_preserve_directory_extensions() {
    let names: Vec<String> = vec!["Photo.JPG".into(), "folder.old".into()];
    let dates = names
        .iter()
        .map(|name| (name.clone(), std::time::UNIX_EPOCH))
        .collect();
    let dirs = HashSet::from(["folder.old".into()]);
    let mut rule = RenameRule {
        parent_affix: Some(ParentAffix::default()),
        prefix: "pre_".into(),
        suffix: "_post".into(),
        extension_case: ExtensionCase::Lower,
        date_stamp: Some(DateStamp {
            format: "%Y_".into(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let plan = |rule: &RenameRule| {
        plan_batch_rename_in_folder(
            &names,
            &HashSet::new(),
            rule,
            &dirs,
            crate::rename_order::NameFolding::Exact,
            &dates,
            Some(std::path::Path::new("/Library/旅行/Album")),
        )
    };
    let first = plan(&rule);
    assert_eq!(first[0].to, "1970_Album_pre_Photo_post.jpg");
    assert_eq!(first[1].to, "1970_Album_pre_folder.old_post");
    rule.parent_affix = Some(ParentAffix {
        depth: 2,
        separator: "-".into(),
        suffix: true,
    });
    assert_eq!(plan(&rule)[0].to, "1970_pre_Photo_post-旅行.jpg");
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(legacy.parent_affix.is_none());
}

#[test]
fn parent_folder_affixes_refuse_missing_levels_and_keep_collision_checks() {
    let names: Vec<String> = vec!["one.txt".into()];
    let mut rule = RenameRule {
        parent_affix: Some(ParentAffix::default()),
        ..Default::default()
    };
    for folder in [None, Some(std::path::Path::new("/"))] {
        let plans = plan_batch_rename_in_folder(
            &names,
            &HashSet::new(),
            &rule,
            &HashSet::new(),
            crate::rename_order::NameFolding::Exact,
            &HashMap::new(),
            folder,
        );
        assert_eq!(plans[0].status, PlanStatus::FolderUnavailable);
        assert!(!plan_is_applicable(&plans));
    }
    for (depth, separator) in [(0, "_"), (33, "_"), (1, "/"), (1, "\n")] {
        rule.parent_affix = Some(ParentAffix {
            depth,
            separator: separator.into(),
            suffix: false,
        });
        assert!(rule.parent_affix.as_ref().unwrap().error().is_some());
    }
    rule.parent_affix = Some(ParentAffix::default());
    let plans = plan_batch_rename_in_folder(
        &names,
        &HashSet::from(["Album_one.txt".into()]),
        &rule,
        &HashSet::new(),
        crate::rename_order::NameFolding::Exact,
        &HashMap::new(),
        Some(std::path::Path::new("/Album")),
    );
    assert_eq!(plans[0].status, PlanStatus::Collision);
    rule.parent_affix.as_mut().unwrap().depth = 3;
    let plans = plan_batch_rename_in_folder(
        &names,
        &HashSet::new(),
        &rule,
        &HashSet::new(),
        crate::rename_order::NameFolding::Exact,
        &HashMap::new(),
        Some(std::path::Path::new("/Album")),
    );
    assert_eq!(plans[0].status, PlanStatus::FolderUnavailable);
}

#[test]
fn captured_date_affixes_follow_original_names_when_reordered() {
    use std::time::{Duration, UNIX_EPOCH};
    let names = vec!["a.JPG".into(), "b.JPG".into(), "folder.old".into()];
    let existing = names.iter().cloned().collect();
    let dirs = HashSet::from(["folder.old".into()]);
    let modified = HashMap::from([
        ("a.JPG".into(), UNIX_EPOCH),
        ("b.JPG".into(), UNIX_EPOCH + Duration::from_secs(86400)),
        (
            "folder.old".into(),
            UNIX_EPOCH + Duration::from_secs(172800),
        ),
    ]);
    let mut rule = RenameRule {
        date_stamp: Some(DateStamp::default()),
        prefix: "pre-".into(),
        suffix: "-post".into(),
        case: CaseMode::Upper,
        extension_case: ExtensionCase::Lower,
        ..Default::default()
    };
    let plan = plan_batch_rename_with_dates(
        &names,
        &existing,
        &rule,
        &dirs,
        crate::rename_order::NameFolding::Exact,
        &modified,
    );
    assert_eq!(
        plan.iter().map(|p| p.to.as_str()).collect::<Vec<_>>(),
        [
            "1970-01-01_pre-A-post.jpg",
            "1970-01-02_pre-B-post.jpg",
            "1970-01-03_pre-FOLDER.OLD-post"
        ]
    );
    assert!(plan_is_applicable(&plan));
    rule.date_stamp = Some(DateStamp {
        format: "_%Y%m%d".into(),
        position: DatePosition::Suffix,
    });
    let reordered = vec![names[1].clone(), names[0].clone()];
    let plan = plan_batch_rename_with_dates(
        &reordered,
        &existing,
        &rule,
        &dirs,
        crate::rename_order::NameFolding::Exact,
        &modified,
    );
    assert_eq!(plan[0].to, "pre-B-post_19700102.jpg");
    assert_eq!(plan[1].to, "pre-A-post_19700101.jpg");
}

#[test]
fn date_rules_refuse_missing_bad_and_unsafe_formats_and_keep_pre_epoch_precision() {
    use std::time::{Duration, UNIX_EPOCH};
    let names = vec!["ёж.txt".into()];
    let existing = names.iter().cloned().collect();
    let dirs = HashSet::new();
    let modified = HashMap::from([("ёж.txt".into(), UNIX_EPOCH - Duration::from_millis(1))]);
    let mut rule = RenameRule {
        date_stamp: Some(DateStamp {
            format: "%s_%f_".into(),
            position: DatePosition::Prefix,
        }),
        ..Default::default()
    };
    let plan = plan_batch_rename_with_dates(
        &names,
        &existing,
        &rule,
        &dirs,
        crate::rename_order::NameFolding::Exact,
        &modified,
    );
    assert_eq!(plan[0].to, "-1_999000000_ёж.txt");
    let missing = plan_batch_rename_with_dates(
        &names,
        &existing,
        &rule,
        &dirs,
        crate::rename_order::NameFolding::Exact,
        &HashMap::new(),
    );
    assert_eq!(missing[0].status, PlanStatus::DateUnavailable);
    assert!(!plan_is_applicable(&missing));
    for format in ["".to_owned(), "%Q".into(), "%#z".into(), "x".repeat(65)] {
        rule.date_stamp.as_mut().unwrap().format = format;
        let plan = plan_batch_rename_with_dates(
            &names,
            &existing,
            &rule,
            &dirs,
            crate::rename_order::NameFolding::Exact,
            &modified,
        );
        assert_eq!(plan[0].status, PlanStatus::DateUnavailable);
        assert!(!plan_is_applicable(&plan));
    }
    rule.date_stamp.as_mut().unwrap().format = "%Y/%m/%d_".into();
    let plan = plan_batch_rename_with_dates(
        &names,
        &existing,
        &rule,
        &dirs,
        crate::rename_order::NameFolding::Exact,
        &modified,
    );
    assert_eq!(plan[0].status, PlanStatus::Invalid);
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(legacy.date_stamp.is_none());
}

#[test]
fn dated_targets_still_use_shared_collision_detection() {
    let names: Vec<String> = vec!["A.txt".into(), "a.txt".into()];
    let existing = names.iter().cloned().collect();
    let modified = names
        .iter()
        .map(|name| (name.clone(), std::time::UNIX_EPOCH))
        .collect();
    let rule = RenameRule {
        date_stamp: Some(DateStamp::default()),
        case: CaseMode::Lower,
        ..Default::default()
    };
    let plan = plan_batch_rename_with_dates(
        &names,
        &existing,
        &rule,
        &HashSet::new(),
        crate::rename_order::NameFolding::Exact,
        &modified,
    );
    assert!(plan.iter().all(|row| row.status == PlanStatus::Collision));
    assert!(!plan_is_applicable(&plan));
}

#[test]
fn replacement_chain_is_ordered_and_composes_with_existing_transforms() {
    let mut rule = RenameRule {
        find: "old".into(),
        replace: "tmp".into(),
        extra_replacements: vec![
            ReplacementPair {
                find: "tmp".into(),
                replace: "final".into(),
            },
            ReplacementPair {
                find: "-".into(),
                replace: "_".into(),
            },
        ],
        replace_target: ReplaceTarget::Stem,
        case: CaseMode::Upper,
        extension_case: ExtensionCase::Lower,
        prefix: "P-".into(),
        suffix: "-S".into(),
        numbering: Some(Numbering {
            start: 1,
            step: 1,
            pad: 2,
        }),
        ..Default::default()
    };
    for (first_only, expected) in [
        (false, "P-FINAL_FINAL-S01.txt"),
        (true, "P-FINAL_OLD-S01.txt"),
    ] {
        rule.first_only = first_only;
        let compiled = compile_find(&rule).unwrap();
        assert_eq!(
            rename_one("old-old.TXT", false, 0, &rule, Some(&compiled)),
            expected
        );
    }
    rule.first_only = false;
    let compiled = compile_find(&rule).unwrap();
    assert_eq!(
        rename_one("old-old.TXT", true, 0, &rule, Some(&compiled)),
        "P-FINAL_FINAL.TXT-S01"
    );
    assert_eq!(
        serde_json::from_str::<RenameRule>(&serde_json::to_string(&rule).unwrap()).unwrap(),
        rule
    );
    assert!(
        !serde_json::to_value(RenameRule::default())
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("extra_replacements")
    );
    assert_eq!(
        serde_json::from_str::<RenameRule>("{}")
            .unwrap()
            .extra_replacements,
        vec![]
    );
}

#[test]
fn replacement_chain_preserves_regex_capture_and_literal_semantics() {
    for (find, replacement, text) in [
        (r"(?P<word>[a-z]+)", "${word}-$0-$$-${missing}", "ab.txt"),
        (r"^|$", "x", "ab"),
        (r"(a)?b", "$1:${1}x:$missing:$$", "b ab"),
    ] {
        let rule = RenameRule {
            regex: true,
            extra_replacements: vec![ReplacementPair {
                find: find.into(),
                replace: replacement.into(),
            }],
            ..Default::default()
        };
        let compiled = compile_find(&rule).unwrap();
        let expected = regex::Regex::new(find)
            .unwrap()
            .replace_all(text, replacement)
            .into_owned();
        assert_eq!(rename_one(text, true, 0, &rule, Some(&compiled)), expected);
    }
    let rule = RenameRule {
        ignore_case: true,
        replace_target: ReplaceTarget::Stem,
        extra_replacements: vec![
            ReplacementPair {
                find: "a".into(),
                replace: "$1".into(),
            },
            ReplacementPair {
                find: "1".into(),
                replace: "Z".into(),
            },
        ],
        ..Default::default()
    };
    let compiled = compile_find(&rule).unwrap();
    assert_eq!(
        rename_one("A-a.txt", false, 0, &rule, Some(&compiled)),
        "$Z-$Z.txt"
    );
    for (target, expected) in [
        (ReplaceTarget::WholeName, "y.y"),
        (ReplaceTarget::Stem, "y.foo"),
        (ReplaceTarget::Extension, "foo.y"),
    ] {
        let rule = RenameRule {
            find: "foo".into(),
            replace: "x".into(),
            extra_replacements: vec![ReplacementPair {
                find: "x".into(),
                replace: "y".into(),
            }],
            replace_target: target,
            ..Default::default()
        };
        let compiled = compile_find(&rule).unwrap();
        assert_eq!(
            rename_one("foo.foo", false, 0, &rule, Some(&compiled)),
            expected
        );
        if target == ReplaceTarget::Extension {
            assert_eq!(
                rename_one("foo.foo", true, 0, &rule, Some(&compiled)),
                "foo.foo"
            );
        }
    }
}

#[test]
fn replacement_chain_limits_block_expansion_without_rejecting_bounded_shrinking() {
    let mut rule = RenameRule {
        regex: true,
        replace_target: ReplaceTarget::Stem,
        extra_replacements: vec![
            ReplacementPair {
                find: ".+".into(),
                replace: "x".repeat(32768),
            },
            ReplacementPair {
                find: "(x+)".into(),
                replace: "$1$1".into(),
            },
            ReplacementPair {
                find: "x+".into(),
                replace: "ok".into(),
            },
        ],
        ..Default::default()
    };
    let names = vec!["a.txt".into()];
    let existing = names.iter().cloned().collect();
    assert!(regex_error(&rule).is_none());
    let plans = plan_batch_rename(&names, &existing, &rule, &HashSet::new());
    assert_eq!(plans[0].to, "ok.txt");
    assert!(plan_is_applicable(&plans));
    rule.extra_replacements[1].replace = "${1}${1}x".into();
    let plans = plan_batch_rename(&names, &existing, &rule, &HashSet::new());
    assert_eq!(plans[0].status, PlanStatus::ExpansionLimit);
    assert!(!plan_is_applicable(&plans));
    let mut rule = RenameRule {
        extra_replacements: vec![ReplacementPair::default()],
        ..Default::default()
    };
    assert!(
        regex_error(&rule)
            .unwrap()
            .contains("Additional replacement 1")
    );
    rule.extra_replacements = vec![
        ReplacementPair {
            find: "a".into(),
            replace: "b".into()
        };
        17
    ];
    assert!(regex_error(&rule).is_some());
    rule.extra_replacements = vec![ReplacementPair {
        find: "a".into(),
        replace: "b".repeat(65536),
    }];
    assert!(regex_error(&rule).is_some());
    rule.extra_replacements = vec![ReplacementPair {
        find: "[".into(),
        replace: "b".into(),
    }];
    rule.regex = true;
    assert!(
        regex_error(&rule)
            .unwrap()
            .contains("Additional replacement 1")
    );
}

#[test]
fn number_position_composes_with_decorations_extensions_and_legacy_rules() {
    let mut rule = RenameRule {
        prefix: "_".into(),
        suffix: "_end".into(),
        case: CaseMode::Upper,
        extension_case: ExtensionCase::Lower,
        numbering: Some(Numbering {
            start: 3,
            step: -2,
            pad: 3,
        }),
        number_position: NumberPosition::BeforeName,
        ..Default::default()
    };
    assert_eq!(
        rename_one("Photo.JPG", false, 0, &rule, None),
        "003_PHOTO_end.jpg"
    );
    assert_eq!(
        rename_one("Photo.JPG", false, 2, &rule, None),
        "-01_PHOTO_end.jpg"
    );
    assert_eq!(rename_one("a.b", true, 0, &rule, None), "003_A.B_end");
    rule.number_position = NumberPosition::AfterName;
    assert_eq!(
        rename_one("Photo.JPG", false, 0, &rule, None),
        "_PHOTO_end003.jpg"
    );
    assert!(
        !serde_json::to_string(&rule)
            .unwrap()
            .contains("number_position")
    );
    let restored: RenameRule =
        serde_json::from_str(r#"{"numbering":{"start":3,"step":1,"pad":2}}"#).unwrap();
    assert_eq!(restored.number_position, NumberPosition::AfterName);
    assert_eq!(rename_one("x.txt", false, 0, &restored, None), "x03.txt");
    rule.numbering = None;
    rule.number_position = NumberPosition::BeforeName;
    assert_eq!(
        rename_one("Photo.JPG", false, 0, &rule, None),
        "_PHOTO_end.jpg"
    );
}

#[test]
fn keep_range_extracts_graphemes_from_either_edge_and_clamps() {
    for (offset, count, from_end, input, expected) in [
        (4, 8, false, "IMG_20261030_raw.jpg", "20261030.jpg"),
        (4, 8, true, "IMG_20261030_raw.jpg", "20261030.jpg"),
        (1, 2, false, "Ae\u{301}👩‍💻Z.txt", "e\u{301}👩‍💻.txt"),
        (1, 2, true, "Ae\u{301}👩‍💻Z.txt", "e\u{301}👩‍💻.txt"),
        (0, usize::MAX, false, "short.txt", "short.txt"),
        (0, usize::MAX, true, "short.txt", "short.txt"),
        (usize::MAX, 2, false, "short.txt", ".txt"),
        (usize::MAX, 2, true, "short.txt", ".txt"),
    ] {
        let rule = RenameRule {
            range_action: RangeAction::Keep,
            remove_range: Some(RemoveRange {
                offset,
                count,
                from_end,
            }),
            ..Default::default()
        };
        assert_eq!(rename_one(input, false, 0, &rule, None), expected);
    }
    let rule = RenameRule {
        range_action: RangeAction::Keep,
        remove_range: Some(RemoveRange {
            offset: 7,
            count: 4,
            from_end: false,
        }),
        ..Default::default()
    };
    assert_eq!(rename_one("folder.name", true, 0, &rule, None), "name");
}

#[test]
fn keep_range_composes_with_scopes_replacement_insertion_and_case() {
    let mut rule = RenameRule {
        range_action: RangeAction::Keep,
        character_target: CharacterTarget::Extension,
        replace_target: ReplaceTarget::Extension,
        find: "old".into(),
        replace: "abcde".into(),
        remove_range: Some(RemoveRange {
            offset: 1,
            count: 2,
            from_end: false,
        }),
        insert_text: Some(InsertText {
            text: "z".into(),
            offset: 2,
            from_end: false,
            overwrite: false,
        }),
        extension_case: ExtensionCase::Upper,
        ..Default::default()
    };
    assert_eq!(
        rename_one("report.old", false, 0, &rule, None),
        "report.BCZ"
    );
    assert_eq!(rename_one("folder.old", true, 0, &rule, None), "folder.old");
    rule.character_target = CharacterTarget::WholeName;
    rule.find.clear();
    rule.insert_text = None;
    rule.extension_case = ExtensionCase::Keep;
    assert_eq!(rename_one("report.txt", false, 0, &rule, None), "ep");
}

#[test]
fn keep_range_preserves_legacy_remove_and_validates_empty_or_duplicate_names() {
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.range_action, RangeAction::Remove);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("range_action")
    );
    let mut rule = RenameRule {
        range_action: RangeAction::Keep,
        remove_range: Some(RemoveRange {
            offset: 1,
            count: 3,
            from_end: false,
        }),
        ..Default::default()
    };
    let plans = plan_files(
        &["xfoo.txt".into(), "yfoo.txt".into()],
        &HashSet::new(),
        &rule,
    );
    assert!(
        plans
            .iter()
            .all(|plan| plan.to == "foo.txt" && plan.status == PlanStatus::Collision)
    );
    rule.character_target = CharacterTarget::WholeName;
    rule.remove_range.as_mut().unwrap().offset = 255;
    assert_eq!(
        plan_files(&["foo.txt".into()], &HashSet::new(), &rule)[0].status,
        PlanStatus::Invalid
    );
}

#[test]
fn letter_counter_boundaries_signs_and_padding_are_explicit() {
    for (value, expected) in [
        (0, "0"),
        (1, "a"),
        (26, "z"),
        (27, "aa"),
        (52, "az"),
        (53, "ba"),
        (702, "zz"),
        (703, "aaa"),
        (-1, "-a"),
        (-27, "-aa"),
    ] {
        assert_eq!(NumberFormat::LowerLetters.render(value, 6), expected);
        assert_eq!(
            NumberFormat::UpperLetters.render(value, 0),
            expected.to_uppercase()
        );
    }
    assert_eq!(NumberFormat::Decimal.render(-7, 4), "-007");
    for value in [i128::MIN, i128::MAX] {
        let result = NumberFormat::LowerLetters.render(value, usize::MAX);
        assert!(result.len() <= 29);
        assert!(
            result
                .trim_start_matches('-')
                .bytes()
                .all(|byte| byte.is_ascii_lowercase())
        );
        assert_eq!(result.starts_with('-'), value < 0);
    }
}

#[test]
fn letter_numbering_composes_with_position_case_and_signed_steps() {
    let mut rule = RenameRule {
        number_format: NumberFormat::LowerLetters,
        numbering: Some(Numbering {
            start: 25,
            step: 1,
            pad: 6,
        }),
        number_position: NumberPosition::CounterOnly,
        ..Default::default()
    };
    for (index, expected) in [(0, "y.txt"), (1, "z.txt"), (2, "aa.txt")] {
        assert_eq!(rename_one("photo.txt", false, index, &rule, None), expected);
    }
    rule.numbering = Some(Numbering {
        start: 1,
        step: -1,
        pad: 2,
    });
    for (index, expected) in [(0, "a.txt"), (1, "0.txt"), (2, "-a.txt")] {
        assert_eq!(rename_one("photo.txt", false, index, &rule, None), expected);
    }
    rule.number_position = NumberPosition::BeforeName;
    rule.prefix = "P-".into();
    rule.suffix = "-S".into();
    rule.case = CaseMode::Upper;
    assert_eq!(
        rename_one("photo.txt", false, 0, &rule, None),
        "aP-PHOTO-S.txt"
    );
    rule.number_format = NumberFormat::UpperLetters;
    rule.number_position = NumberPosition::AfterName;
    assert_eq!(
        rename_one("photo.txt", false, 0, &rule, None),
        "P-PHOTO-SA.txt"
    );
}

#[test]
fn letter_counters_keep_legacy_defaults_and_collision_guards() {
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.number_format, NumberFormat::Decimal);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("number_format")
    );
    let rule = RenameRule {
        number_format: NumberFormat::LowerLetters,
        numbering: Some(Numbering {
            start: 1,
            step: 0,
            pad: 2,
        }),
        number_position: NumberPosition::CounterOnly,
        ..Default::default()
    };
    let plans = plan_files(
        &["one.txt".into(), "two.txt".into()],
        &HashSet::new(),
        &rule,
    );
    assert!(
        plans
            .iter()
            .all(|plan| plan.to == "a.txt" && plan.status == PlanStatus::Collision)
    );
    assert!(!plan_is_applicable(&plans));
    let wide = RenameRule {
        numbering: Some(Numbering {
            start: i64::MIN,
            step: i64::MIN,
            pad: 0,
        }),
        ..rule
    };
    let output = rename_one("photo.txt", false, usize::MAX, &wide, None);
    assert!(output.starts_with('-') && output.ends_with(".txt") && output.len() < 40);
}

#[test]
fn character_edit_scopes_handle_extensions_folders_and_dotfiles() {
    for (target, input, is_dir, expected) in [
        (CharacterTarget::Stem, "report.txt", false, "_report.txt"),
        (
            CharacterTarget::Extension,
            "report.txt",
            false,
            "report._txt",
        ),
        (
            CharacterTarget::WholeName,
            "report.txt",
            false,
            "_report.txt",
        ),
        (CharacterTarget::Extension, "report", false, "report._"),
        (CharacterTarget::Extension, ".hidden", false, ".hidden._"),
        (
            CharacterTarget::Extension,
            "folder.name",
            true,
            "folder.name",
        ),
        (
            CharacterTarget::WholeName,
            "folder.name",
            true,
            "_folder.name",
        ),
        (
            CharacterTarget::Extension,
            "archive.tar.gz",
            false,
            "archive.tar._gz",
        ),
    ] {
        let rule = RenameRule {
            character_target: target,
            insert_text: Some(InsertText {
                text: "_".into(),
                offset: 0,
                from_end: false,
                overwrite: false,
            }),
            ..Default::default()
        };
        assert_eq!(rename_one(input, is_dir, 0, &rule, None), expected);
    }
    let no_op = RenameRule {
        character_target: CharacterTarget::Extension,
        insert_text: Some(InsertText {
            text: String::new(),
            offset: 0,
            from_end: false,
            overwrite: true,
        }),
        ..Default::default()
    };
    assert_eq!(rename_one("report.", false, 0, &no_op, None), "report.");
}

#[test]
fn character_edits_compose_with_replacement_and_extension_case() {
    let rule = RenameRule {
        character_target: CharacterTarget::Extension,
        replace_target: ReplaceTarget::Extension,
        find: "old".into(),
        replace: "txt".into(),
        remove_range: Some(RemoveRange {
            offset: 0,
            count: 1,
            from_end: false,
        }),
        insert_text: Some(InsertText {
            text: "p".into(),
            offset: 0,
            from_end: false,
            overwrite: false,
        }),
        extension_case: ExtensionCase::Upper,
        case: CaseMode::Lower,
        prefix: "P-".into(),
        ..Default::default()
    };
    assert_eq!(
        rename_one("Report.old", false, 0, &rule, None),
        "P-report.PXT"
    );
    let whole = RenameRule {
        character_target: CharacterTarget::WholeName,
        remove_range: Some(RemoveRange {
            offset: 6,
            count: 1,
            from_end: false,
        }),
        ..Default::default()
    };
    assert_eq!(
        rename_one("report.txt", false, 0, &whole, None),
        "reporttxt"
    );
    let unicode = RenameRule {
        character_target: CharacterTarget::Extension,
        remove_range: Some(RemoveRange {
            offset: 0,
            count: 1,
            from_end: false,
        }),
        ..Default::default()
    };
    assert_eq!(
        rename_one("name.e\u{301}👩‍💻", false, 0, &unicode, None),
        "name.👩‍💻"
    );
}

#[test]
fn character_scope_defaults_and_validation_preserve_existing_guards() {
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.character_target, CharacterTarget::Stem);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("character_target")
    );
    let mut rule = RenameRule {
        character_target: CharacterTarget::Extension,
        remove_range: Some(RemoveRange {
            offset: 0,
            count: 255,
            from_end: false,
        }),
        ..Default::default()
    };
    let plans = plan_files(&["a.txt".into(), "a.md".into()], &HashSet::new(), &rule);
    assert!(
        plans
            .iter()
            .all(|plan| plan.to == "a" && plan.status == PlanStatus::Collision)
    );
    rule.character_target = CharacterTarget::WholeName;
    assert_eq!(
        plan_files(&["foo.txt".into()], &HashSet::new(), &rule)[0].status,
        PlanStatus::Invalid
    );
}

#[test]
fn insertion_and_overwrite_use_whole_graphemes_and_clamp_anchors() {
    for (offset, from_end, overwrite, text, input, expected) in [
        (2, false, false, "X", "abcdef.txt", "abXcdef.txt"),
        (2, true, false, "X", "abcdef.txt", "abcdXef.txt"),
        (1, false, true, "文", "Ae\u{301}👩‍💻Z.jpg", "A文👩‍💻Z.jpg"),
        (2, true, true, "👨‍👩‍👧", "Ae\u{301}👩‍💻Z.jpg", "Ae\u{301}👨‍👩‍👧Z.jpg"),
        (usize::MAX, false, false, "X", "a.txt", "aX.txt"),
        (usize::MAX, true, true, "YZ", "a.txt", "YZ.txt"),
        (1, false, true, "LONG", "ab.txt", "aLONG.txt"),
        (0, false, true, "", "a.txt", "a.txt"),
        (0, false, false, "$1", ".hidden", "$1.hidden"),
        (0, true, false, "_", "archive.tar.gz", "archive.tar_.gz"),
    ] {
        let rule = RenameRule {
            insert_text: Some(InsertText {
                offset,
                from_end,
                overwrite,
                text: text.into(),
            }),
            ..Default::default()
        };
        assert_eq!(rename_one(input, false, 0, &rule, None), expected);
    }
    let rule = RenameRule {
        insert_text: Some(InsertText {
            offset: 0,
            from_end: true,
            overwrite: false,
            text: "_".into(),
        }),
        ..Default::default()
    };
    assert_eq!(
        rename_one("folder.name", true, 0, &rule, None),
        "folder.name_"
    );
}

#[test]
fn insertion_composes_after_removal_before_case_and_decoration() {
    let rule = RenameRule {
        remove_range: Some(RemoveRange {
            offset: 1,
            count: 2,
            from_end: false,
        }),
        insert_text: Some(InsertText {
            text: "z".into(),
            offset: 1,
            from_end: false,
            overwrite: false,
        }),
        find: "old".into(),
        replace: "abcde".into(),
        case: CaseMode::Upper,
        prefix: "P-".into(),
        suffix: "-S".into(),
        numbering: Some(Numbering {
            start: 7,
            step: 1,
            pad: 2,
        }),
        ..Default::default()
    };
    assert_eq!(
        rename_one("old.txt", false, 0, &rule, None),
        "P-AZDE-S07.txt"
    );
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(legacy.insert_text.is_none());
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("insert_text")
    );
}

#[test]
fn inserted_invalid_names_and_overwrite_collisions_are_blocked() {
    let mut rule = RenameRule {
        insert_text: Some(InsertText {
            text: "z".into(),
            offset: 0,
            from_end: false,
            overwrite: true,
        }),
        ..Default::default()
    };
    let inputs = ["xfoo.txt".into(), "yfoo.txt".into()];
    let plans = plan_files(&inputs, &HashSet::new(), &rule);
    assert!(
        plans
            .iter()
            .all(|plan| plan.status == PlanStatus::Collision)
    );
    assert!(!plan_is_applicable(&plans));
    rule.insert_text.as_mut().unwrap().text = "/".into();
    assert!(
        plan_files(&inputs, &HashSet::new(), &rule)
            .iter()
            .all(|plan| plan.status == PlanStatus::Invalid)
    );
    rule.insert_text.as_mut().unwrap().text = "a".repeat(256);
    assert!(
        plan_files(&inputs, &HashSet::new(), &rule)
            .iter()
            .all(|plan| plan.status == PlanStatus::Invalid)
    );
}

#[test]
fn remove_ranges_keep_graphemes_extensions_and_clamp_from_both_edges() {
    for (offset, count, from_end, input, expected) in [
        (1, 2, false, "abcdef.txt", "adef.txt"),
        (1, 2, true, "abcdef.txt", "abcf.txt"),
        (1, 1, false, "Ae\u{301}👩‍💻Z.jpg", "A👩‍💻Z.jpg"),
        (1, 1, true, "Ae\u{301}👩‍💻Z.jpg", "Ae\u{301}Z.jpg"),
        (99, usize::MAX, false, "short.txt", "short.txt"),
        (99, usize::MAX, true, "short.txt", "short.txt"),
        (1, usize::MAX, false, "abc.txt", "a.txt"),
        (1, usize::MAX, true, "abc.txt", "c.txt"),
        (0, 1, false, ".hidden", "hidden"),
        (0, 2, true, "archive.tar.gz", "archive.t.gz"),
    ] {
        let rule = RenameRule {
            remove_range: Some(RemoveRange {
                offset,
                count,
                from_end,
            }),
            ..Default::default()
        };
        assert_eq!(rename_one(input, false, 0, &rule, None), expected);
    }
    let rule = RenameRule {
        remove_range: Some(RemoveRange {
            offset: 1,
            count: 2,
            from_end: true,
        }),
        ..Default::default()
    };
    assert_eq!(rename_one("folder.name", true, 0, &rule, None), "folder.ne");
}

#[test]
fn remove_range_composes_after_replacements_before_decoration_and_numbering() {
    let rule = RenameRule {
        remove_range: Some(RemoveRange {
            offset: 1,
            count: 2,
            from_end: false,
        }),
        find: "old".into(),
        replace: "abcde".into(),
        case: CaseMode::Upper,
        prefix: "P-".into(),
        suffix: "-S".into(),
        numbering: Some(Numbering {
            start: 7,
            step: 1,
            pad: 2,
        }),
        ..Default::default()
    };
    assert_eq!(
        rename_one("old.txt", false, 0, &rule, None),
        "P-ADE-S07.txt"
    );
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(legacy.remove_range.is_none());
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("remove_range")
    );
}

#[test]
fn remove_range_reports_empty_and_duplicate_results() {
    let rule = RenameRule {
        remove_range: Some(RemoveRange {
            offset: 0,
            count: 1,
            from_end: false,
        }),
        ..Default::default()
    };
    let plans = plan_files(
        &["a".into(), "xfoo.txt".into(), "yfoo.txt".into()],
        &HashSet::new(),
        &rule,
    );
    assert_eq!(plans[0].status, PlanStatus::Invalid);
    assert_eq!(plans[1].status, PlanStatus::Collision);
    assert_eq!(plans[2].status, PlanStatus::Collision);
    assert!(!plan_is_applicable(&plans));
}

#[test]
fn stem_limit_preserves_extension_and_whole_graphemes_and_checks_collisions() {
    let rule = RenameRule {
        stem_limit: Some(2),
        ..Default::default()
    };
    for (input, expected) in [
        ("abcdef.txt", "ab.txt"),
        ("e\u{301}👩‍💻rest.jpg", "e\u{301}👩‍💻.jpg"),
        ("a.txt", "a.txt"),
        (".hidden", ".h"),
        ("archive.tar.gz", "ar.gz"),
    ] {
        assert_eq!(rename_one(input, false, 0, &rule, None), expected);
    }
    assert_eq!(rename_one("folder.name", true, 0, &rule, None), "fo");
    let decorated = RenameRule {
        prefix: "XY".into(),
        ..rule.clone()
    };
    assert_eq!(
        rename_one("abcdef.txt", false, 0, &decorated, None),
        "XY.txt"
    );
    let plans = plan_files(
        &["abc.txt".into(), "abd.txt".into()],
        &HashSet::new(),
        &rule,
    );
    assert!(
        plans
            .iter()
            .all(|plan| plan.status == PlanStatus::Collision)
    );
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.stem_limit, None);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("stem_limit")
    );
    assert_eq!(
        rename_one("abcdef.txt", false, 0, &legacy, None),
        "abcdef.txt"
    );
}

#[test]
fn remove_affixes_is_exact_once_before_extension_and_other_transforms() {
    let rule = RenameRule {
        remove_prefix: "backup_".into(),
        remove_suffix: ".old".into(),
        extension_case: ExtensionCase::Upper,
        ..Default::default()
    };
    assert_eq!(
        rename_one("backup_photo.jpg.old", false, 0, &rule, None),
        "photo.JPG"
    );
    assert_eq!(
        rename_one("backup_backup_photo.jpg.old.old", false, 0, &rule, None),
        "backup_photo.jpg.OLD"
    );
    assert_eq!(
        rename_one("BACKUP_photo.jpg.OLD", false, 0, &rule, None),
        "BACKUP_photo.jpg.OLD"
    );
    assert_eq!(rename_one("backup_目录.old", true, 0, &rule, None), "目录");
    let decorated = RenameRule {
        prefix: "new_".into(),
        suffix: "_done".into(),
        ..rule.clone()
    };
    assert_eq!(
        rename_one("backup_photo.jpg.old", false, 0, &decorated, None),
        "new_photo_done.JPG"
    );
    let names = vec![
        "backup_photo.jpg.old".into(),
        "photo.JPG".into(),
        "backup_.old".into(),
    ];
    let plans = plan_files(&names, &HashSet::new(), &rule);
    assert_eq!(plans[0].status, PlanStatus::Collision);
    assert_eq!(plans[2].status, PlanStatus::Invalid);
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(legacy.remove_prefix.is_empty() && legacy.remove_suffix.is_empty());
    assert!(!serde_json::to_string(&legacy).unwrap().contains("remove_"));
}

#[test]
fn normalization_forms_apply_after_decoration_and_reject_invalid_results() {
    for (normalization, input, expected) in [
        (NameNormalization::Nfc, "cafe\u{301}.txt", "café.txt"),
        (NameNormalization::Nfd, "café.txt", "cafe\u{301}.txt"),
        (NameNormalization::Nfkc, "Ｆｉｌｅ①.txt", "File1.txt"),
        (NameNormalization::Nfkd, "é①.txt", "e\u{301}1.txt"),
    ] {
        let rule = RenameRule {
            normalization,
            ..Default::default()
        };
        assert_eq!(rename_one(input, false, 0, &rule, None), expected);
        assert_eq!(rename_one(input, true, 0, &rule, None), expected);
        assert_eq!(rename_one(expected, false, 0, &rule, None), expected);
    }
    let rule = RenameRule {
        suffix: "\u{301}".into(),
        normalization: NameNormalization::Nfc,
        ..Default::default()
    };
    assert_eq!(rename_one("cafe", false, 0, &rule, None), "café");
    let rule = RenameRule {
        normalization: NameNormalization::Nfkc,
        ..Default::default()
    };
    let names = vec![
        "a／b.txt".into(),
        "．．".into(),
        "Ｆｉｌｅ.txt".into(),
        "File.txt".into(),
    ];
    let plans = plan_files(&names, &HashSet::new(), &rule);
    assert_eq!(plans[0].status, PlanStatus::Invalid);
    assert_eq!(plans[1].status, PlanStatus::Invalid);
    assert_eq!(plans[2].status, PlanStatus::Collision);
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.normalization, NameNormalization::Keep);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("normalization")
    );
    assert_eq!(
        rename_one("cafe\u{301}.txt", false, 0, &legacy, None),
        "cafe\u{301}.txt"
    );
}

#[test]
fn collapse_repeats_preserves_letters_and_unicode_clusters_and_checks_collisions() {
    let rule = RenameRule {
        collapse_repeats: true,
        ..Default::default()
    };
    for (name, expected) in [
        ("book___final--v22..txt", "book_final-v22.txt"),
        ("a  b__--c", "a b_-c"),
        ("éé_e\u{301}e\u{301}__文件11", "éé_e\u{301}e\u{301}_文件11"),
        ("a👩‍💻👩‍💻b", "a👩‍💻b"),
    ] {
        assert_eq!(rename_one(name, false, 0, &rule, None), expected);
        assert_eq!(rename_one(name, true, 0, &rule, None), expected);
    }
    let decorated = RenameRule {
        prefix: "__".into(),
        suffix: "__".into(),
        trim_spaces: true,
        ..rule.clone()
    };
    assert_eq!(
        rename_one("_folder_ ", true, 0, &decorated, None),
        "_folder_ _"
    );
    let names = vec!["a__b.txt".into(), "a_b.txt".into(), "...".into()];
    let plans = plan_files(&names, &HashSet::new(), &rule);
    assert_eq!(plans[0].status, PlanStatus::Collision);
    assert_eq!(plans[2].status, PlanStatus::Invalid);
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(!legacy.collapse_repeats);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("collapse_repeats")
    );
    assert_eq!(rename_one("a__b", false, 0, &legacy, None), "a__b");
}

#[test]
fn trim_spaces_cleans_final_names_and_reports_invalid_or_colliding_results() {
    let rule = RenameRule {
        trim_spaces: true,
        ..Default::default()
    };
    for (name, expected) in [
        (" Photo.JPG ", "Photo.JPG"),
        ("\u{2003}a b.txt\t", "a b.txt"),
        ("folder name ", "folder name"),
    ] {
        assert_eq!(rename_one(name, false, 0, &rule, None), expected);
        assert_eq!(rename_one(name, true, 0, &rule, None), expected);
    }
    let decorated = RenameRule {
        prefix: "  ".into(),
        suffix: "  ".into(),
        ..rule.clone()
    };
    assert_eq!(rename_one("folder", true, 0, &decorated, None), "folder");
    let names = vec![" a.txt ".into(), "a.txt".into(), "   ".into()];
    let plans = plan_files(&names, &HashSet::new(), &rule);
    assert_eq!(plans[0].status, PlanStatus::Collision);
    assert_eq!(plans[2].status, PlanStatus::Invalid);
    let legacy: RenameRule = serde_json::from_str("{}").unwrap();
    assert!(!legacy.trim_spaces);
    assert_eq!(rename_one(" a.txt ", false, 0, &legacy, None), " a.txt ");
}

#[test]
fn counter_only_replaces_decorated_stem_and_retains_extension() {
    let mut rule = RenameRule {
        prefix: "ignored_".into(),
        suffix: "_ignored".into(),
        number_position: NumberPosition::CounterOnly,
        extension_case: ExtensionCase::Lower,
        numbering: Some(Numbering {
            start: 1,
            step: -1,
            pad: 3,
        }),
        ..Default::default()
    };
    assert_eq!(rename_one("Photo.JPG", false, 0, &rule, None), "001.jpg");
    assert_eq!(rename_one("Photo.JPG", false, 2, &rule, None), "-01.jpg");
    assert_eq!(rename_one("folder.with.dots", true, 0, &rule, None), "001");
    assert_eq!(rename_one("LICENSE", false, 0, &rule, None), "001");
    let names = vec!["a.txt".to_string(), "b.txt".to_string()];
    rule.numbering.as_mut().unwrap().step = 0;
    let plans = plan_files(&names, &HashSet::new(), &rule);
    assert!(
        plans
            .iter()
            .all(|plan| plan.status == PlanStatus::Collision)
    );
    rule.numbering = None;
    assert_eq!(
        rename_one("Photo.JPG", false, 0, &rule, None),
        "ignored_Photo_ignored.jpg"
    );
}

#[test]
fn signed_numbering_descends_crosses_zero_and_pads_the_sign() {
    let rule = RenameRule {
        numbering: Some(Numbering {
            start: 3,
            step: -2,
            pad: 3,
        }),
        ..Default::default()
    };
    for (index, expected) in [
        "Photo003.JPG",
        "Photo001.JPG",
        "Photo-01.JPG",
        "Photo-03.JPG",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            rename_one("Photo.JPG", false, index, &rule, None),
            *expected
        );
    }
    let mut negative = rule.clone();
    negative.numbering.as_mut().unwrap().start = -12;
    assert_eq!(rename_one("folder", true, 1, &negative, None), "folder-14");
    negative.numbering.as_mut().unwrap().step = 0;
    assert_eq!(rename_one("a", false, 99, &negative, None), "a-12");
}

#[test]
fn signed_numbering_widens_arithmetic_and_accepts_legacy_unsigned_values() {
    let legacy: Numbering =
        serde_json::from_str(r#"{"start":4294967295,"step":4294967295,"pad":0}"#).unwrap();
    let rule = RenameRule {
        numbering: Some(legacy),
        ..Default::default()
    };
    assert_eq!(rename_one("x", false, 1, &rule, None), "x8589934590");
    for (start, step) in [(i64::MAX, i64::MAX), (i64::MIN, i64::MIN)] {
        let rule = RenameRule {
            numbering: Some(Numbering {
                start,
                step,
                pad: 0,
            }),
            ..Default::default()
        };
        let expected = i128::from(start) + usize::MAX as i128 * i128::from(step);
        assert_eq!(
            rename_one("x", false, usize::MAX, &rule, None),
            format!("x{expected}")
        );
    }
}

#[test]
fn extension_case_changes_only_the_last_extension_after_find_replace() {
    for (source, lower, upper) in [
        ("Photo.JPG", "Photo.jpg", "Photo.JPG"),
        ("archive.TAR.GZ", "archive.TAR.gz", "archive.TAR.GZ"),
        (".ENV", ".ENV", ".ENV"),
        ("config.É", "config.é", "config.É"),
        ("file.ß", "file.ß", "file.SS"),
        ("plain", "plain", "plain"),
        ("trailing.", "trailing.", "trailing."),
    ] {
        for (mode, expected) in [(ExtensionCase::Lower, lower), (ExtensionCase::Upper, upper)] {
            let rule = RenameRule {
                extension_case: mode,
                ..Default::default()
            };
            assert_eq!(rename_one(source, false, 0, &rule, None), expected);
            assert_eq!(
                rename_one(source, true, 0, &rule, None),
                source,
                "directories have no extension"
            );
        }
    }
    let rule = RenameRule {
        find: "JPG".into(),
        replace: "PNG".into(),
        extension_case: ExtensionCase::Lower,
        case: CaseMode::Upper,
        prefix: "p_".into(),
        numbering: Some(Numbering {
            start: 2,
            step: 1,
            pad: 1,
        }),
        ..Default::default()
    };
    assert_eq!(
        rename_one("Photo.JPG", false, 0, &rule, None),
        "p_PHOTO2.png"
    );
    let rule = RenameRule {
        replace_target: ReplaceTarget::Extension,
        ..rule
    };
    assert_eq!(
        rename_one("Photo.JPG", false, 0, &rule, None),
        "p_PHOTO2.png"
    );
}

#[test]
fn extension_case_defaults_preserve_legacy_json_and_collisions_are_blocked() {
    let legacy: RenameRule = serde_json::from_str(r#"{"case":"Lower"}"#).unwrap();
    assert_eq!(legacy.extension_case, ExtensionCase::Keep);
    assert!(
        !serde_json::to_string(&legacy)
            .unwrap()
            .contains("extension_case")
    );
    let rule = RenameRule {
        extension_case: ExtensionCase::Lower,
        ..Default::default()
    };
    let plan = plan_files(
        &names(&["a.TXT", "a.txt"]),
        &existing(&["a.TXT", "a.txt"]),
        &rule,
    );
    assert_eq!(plan[0].status, PlanStatus::Collision);
    assert_eq!(plan[1].status, PlanStatus::Unchanged);
    assert!(!plan_is_applicable(&plan));
    assert!(serde_json::from_str::<RenameRule>(r#"{"extension_case":"TitleWords"}"#).is_err());
}

#[test]
fn capitalization_modes_use_unicode_and_preserve_word_separators() {
    for (input, capital, title) in [
        ("hELLO wORLD", "Hello world", "Hello World"),
        ("пРИВЕТ-мИР_тЕСТ", "Привет-мир_тест", "Привет-Мир_тест"),
        (
            "e\u{301}COLE café",
            "E\u{301}cole café",
            "E\u{301}cole Café",
        ),
        ("ßETA TEST", "SSeta test", "SSeta Test"),
        (
            "hello  WORLD\tAGAIN",
            "Hello  world\tagain",
            "Hello  World\tAgain",
        ),
        ("", "", ""),
    ] {
        assert_eq!(apply_case(input, CaseMode::Capitalize), capital, "{input}");
        assert_eq!(apply_case(input, CaseMode::TitleWords), title, "{input}");
    }
}

#[test]
fn capitalization_keeps_extension_and_existing_transform_order() {
    for (mode, expected) in [
        (CaseMode::Capitalize, "PRE_Hello world_END07.TXT"),
        (CaseMode::TitleWords, "PRE_Hello World_END07.TXT"),
    ] {
        let rule = RenameRule {
            find: "OLD".into(),
            replace: "hELLO".into(),
            case: mode,
            prefix: "PRE_".into(),
            suffix: "_END".into(),
            numbering: Some(Numbering {
                start: 7,
                step: 1,
                pad: 2,
            }),
            ..Default::default()
        };
        let plans = plan_files(
            &names(&["OLD wORLD.TXT"]),
            &existing(&["OLD wORLD.TXT"]),
            &rule,
        );
        assert_eq!(plans[0].to, expected);
        assert_eq!(plans[0].status, PlanStatus::Ok);
        assert_eq!(
            rename_one(
                "hELLO.wORLD",
                true,
                0,
                &RenameRule {
                    case: mode,
                    ..Default::default()
                },
                None
            ),
            "Hello.world"
        );
    }
}
#[test]
fn first_only_and_ignore_case_compose_without_expanding_literal_replacements() {
    let input = names(&["Aa-aA-aa.txt"]);
    let existing = existing(&["Aa-aA-aa.txt"]);
    let mut rule = RenameRule {
        find: "aa".into(),
        replace: "x".into(),
        ..Default::default()
    };
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Aa-aA-x.txt");
    rule.ignore_case = true;
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "x-x-x.txt");
    rule.first_only = true;
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "x-aA-aa.txt");
    rule.ignore_case = false;
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Aa-aA-x.txt");
    rule.ignore_case = true;
    rule.replace = "$1${name}".into();
    assert_eq!(
        plan_files(&input, &existing, &rule)[0].to,
        "$1${name}-aA-aa.txt"
    );
    rule.first_only = false;
    assert_eq!(
        plan_files(&input, &existing, &rule)[0].to,
        "$1${name}-$1${name}-$1${name}.txt"
    );
    rule.regex = true;
    rule.first_only = true;
    rule.find = "(aa)".into();
    rule.replace = "$1_$1".into();
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Aa-aA-aa.txt");
    // Explicit braces disambiguate a capture reference from a longer name.
    rule.replace = "${1}_${1}".into();
    assert_eq!(
        plan_files(&input, &existing, &rule)[0].to,
        "Aa_Aa-aA-aa.txt"
    );
}

#[test]
fn matching_options_use_unicode_simple_folding_and_respect_component_scope() {
    let input = names(&["Ёж-ёж.ЁЖ"]);
    let existing = existing(&["Ёж-ёж.ЁЖ"]);
    let mut rule = RenameRule {
        find: "ёж".into(),
        replace: "ёжик".into(),
        ignore_case: true,
        first_only: true,
        ..Default::default()
    };
    rule.replace_target = ReplaceTarget::Extension;
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "Ёж-ёж.ёжик");
    rule.replace_target = ReplaceTarget::Stem;
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "ёжик-ёж.ЁЖ");
    rule.replace_target = ReplaceTarget::WholeName;
    rule.first_only = false;
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "ёжик-ёжик.ёжик");
    let input = names(&["Straße.txt"]);
    rule.find = "STRASSE".into();
    rule.replace = "Road".into();
    assert_eq!(
        plan_files(&input, &HashSet::new(), &rule)[0].to,
        "Straße.txt"
    );
}

#[test]
fn first_only_handles_zero_width_regex_but_empty_find_keeps_legacy_noop() {
    let input = names(&["ёж.txt"]);
    let existing = existing(&["ёж.txt"]);
    let mut rule = RenameRule {
        find: "(?:)".into(),
        replace: "-".into(),
        regex: true,
        replace_target: ReplaceTarget::Stem,
        first_only: true,
        ..Default::default()
    };
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "-ёж.txt");
    rule.first_only = false;
    assert_eq!(plan_files(&input, &existing, &rule)[0].to, "-ё-ж-.txt");
    rule.find.clear();
    rule.first_only = true;
    rule.ignore_case = true;
    assert_eq!(
        plan_files(&input, &existing, &rule)[0].status,
        PlanStatus::Unchanged
    );
    let old: RenameRule = serde_json::from_str(r#"{"find":"a","replace":"b"}"#).unwrap();
    assert!(!old.ignore_case && !old.first_only);
    let encoded = serde_json::to_value(old).unwrap();
    assert!(encoded.get("ignore_case").is_none() && encoded.get("first_only").is_none());
}

#[test]
fn replacement_target_distinguishes_whole_stem_and_extension() {
    let input = names(&["txt-report.txt"]);
    let existing = existing(&["txt-report.txt"]);
    let mut rule = RenameRule {
        find: "txt".into(),
        replace: "log".into(),
        ..Default::default()
    };
    for (target, expected) in [
        (ReplaceTarget::WholeName, "log-report.log"),
        (ReplaceTarget::Stem, "log-report.txt"),
        (ReplaceTarget::Extension, "txt-report.log"),
    ] {
        rule.replace_target = target;
        let plan = plan_files(&input, &existing, &rule);
        assert_eq!(plan[0].to, expected);
        assert_eq!(plan[0].status, PlanStatus::Ok);
    }
    rule.replace_target = ReplaceTarget::Stem;
    rule.replace = "log.md".into();
    assert_eq!(
        plan_files(&input, &existing, &rule)[0].to,
        "log.md-report.txt"
    );
}

#[test]
fn extension_target_supports_removal_creation_and_identity_without_affecting_folders() {
    let input = names(&[
        "report.txt",
        ".gitignore",
        "archive.tar.gz",
        "trailing.",
        "plain",
        "folder.v1",
    ]);
    let existing: HashSet<_> = input.iter().cloned().collect();
    let dirs = names(&["folder.v1"]).into_iter().collect();
    let mut rule = RenameRule {
        replace_target: ReplaceTarget::Extension,
        ..Default::default()
    };
    let plans = plan_batch_rename(&input, &existing, &rule, &dirs);
    assert!(
        plans
            .iter()
            .all(|plan| plan.status == PlanStatus::Unchanged)
    );
    rule.find = "txt".into();
    rule.replace = String::new();
    assert_eq!(
        plan_batch_rename(&input, &existing, &rule, &dirs)[0].to,
        "report"
    );
    rule.find = "^$".into();
    rule.replace = "txt".into();
    rule.regex = true;
    let plans = plan_batch_rename(&input, &existing, &rule, &dirs);
    assert_eq!(plans[1].to, ".gitignore.txt");
    assert_eq!(plans[3].to, "trailing.txt");
    assert_eq!(plans[4].to, "plain.txt");
    assert_eq!(plans[5].to, "folder.v1");
    rule.find = "gz".into();
    rule.replace = "zip".into();
    rule.prefix = "x_".into();
    let plans = plan_batch_rename(&input, &existing, &rule, &dirs);
    assert_eq!(plans[2].to, "x_archive.tar.zip");
    assert_eq!(plans[5].to, "x_folder.v1");
}

#[test]
fn targeted_regex_groups_structural_edits_and_collisions_use_the_same_planner() {
    let input = names(&["item12.JPG"]);
    let mut rule = RenameRule {
        find: r"item(\d+)".into(),
        replace: "photo-$1".into(),
        regex: true,
        replace_target: ReplaceTarget::Stem,
        suffix: "_end".into(),
        case: CaseMode::Upper,
        numbering: Some(Numbering {
            start: 1,
            step: 1,
            pad: 2,
        }),
        ..Default::default()
    };
    let plans = plan_files(&input, &existing(&["item12.JPG"]), &rule);
    assert_eq!(plans[0].to, "PHOTO-12_end01.JPG");
    rule = RenameRule {
        find: "JPG".into(),
        replace: "png".into(),
        replace_target: ReplaceTarget::Extension,
        ..Default::default()
    };
    assert_eq!(
        plan_files(&input, &existing(&["item12.JPG", "item12.png"]), &rule)[0].status,
        PlanStatus::Collision
    );
    rule.replace = "../png".into();
    assert_eq!(
        plan_files(&input, &existing(&["item12.JPG"]), &rule)[0].status,
        PlanStatus::Invalid
    );
}

#[test]
fn missing_target_deserializes_to_legacy_whole_name_and_unknown_targets_fail() {
    let rule: RenameRule = serde_json::from_str(r#"{"find":"txt","replace":"log"}"#).unwrap();
    assert_eq!(rule.replace_target, ReplaceTarget::WholeName);
    assert!(
        serde_json::to_value(&rule)
            .unwrap()
            .get("replace_target")
            .is_none()
    );
    assert!(serde_json::from_str::<RenameRule>(r#"{"replace_target":"Unexpected"}"#).is_err());
}

#[test]
fn folder_names_have_no_extension_to_preserve() {
    let rule = RenameRule {
        suffix: "_old".into(),
        case: CaseMode::Upper,
        ..Default::default()
    };
    let n = names(&["v1.2", "notes.txt"]);
    let ex = existing(&["v1.2", "notes.txt"]);
    let plans = plan_batch_rename(&n, &ex, &rule, &existing(&["v1.2"]));
    assert_eq!(plans[0].to, "V1.2_old");
    assert_eq!(plans[1].to, "NOTES_old.txt");
}

#[test]
fn collision_policy_distinguishes_case_sensitive_directories() {
    let selected = names(&["bar"]);
    let siblings = existing(&["Foo", "bar"]);
    let rule = RenameRule {
        find: "bar".into(),
        replace: "foo".into(),
        ..Default::default()
    };

    let folded = plan_batch_rename_with(
        &selected,
        &siblings,
        &rule,
        &HashSet::new(),
        crate::rename_order::NameFolding::CaseFolded,
    );
    let exact = plan_batch_rename_with(
        &selected,
        &siblings,
        &rule,
        &HashSet::new(),
        crate::rename_order::NameFolding::Exact,
    );

    assert_eq!(folded[0].status, PlanStatus::Collision);
    assert_eq!(exact[0].status, PlanStatus::Ok);
}

#[test]
fn find_replace_touches_whole_name_then_splits_extension() {
    let rule = RenameRule {
        find: "IMG".into(),
        replace: "Photo".into(),
        ..Default::default()
    };
    let n = names(&["IMG_01.jpg", "IMG_02.jpg"]);
    let ex = existing(&["IMG_01.jpg", "IMG_02.jpg"]);
    let plans = plan_files(&n, &ex, &rule);
    assert_eq!(plans[0].to, "Photo_01.jpg");
    assert_eq!(plans[1].to, "Photo_02.jpg");
    assert_eq!(plans[0].status, PlanStatus::Ok);
}

#[test]
fn regex_find_replace_expands_capture_groups() {
    let rule = RenameRule {
        find: r"IMG_(\d+)".into(),
        replace: "Photo-$1".into(),
        regex: true,
        ..Default::default()
    };
    let n = names(&["IMG_042.jpg", "IMG_007.jpg"]);
    let ex = existing(&["IMG_042.jpg", "IMG_007.jpg"]);
    let plans = plan_files(&n, &ex, &rule);
    assert_eq!(plans[0].to, "Photo-042.jpg");
    assert_eq!(plans[1].to, "Photo-007.jpg");
}

#[test]
fn regex_error_reports_an_invalid_pattern() {
    let rule = RenameRule {
        find: "(unclosed".into(),
        regex: true,
        ..Default::default()
    };
    assert!(regex_error(&rule).is_some());
}

#[test]
fn regex_error_is_none_outside_regex_mode_or_with_an_empty_pattern() {
    assert_eq!(regex_error(&RenameRule::default()), None);
    let literal = RenameRule {
        find: "(unclosed".into(), // invalid as regex, fine as a literal
        ..Default::default()
    };
    assert_eq!(regex_error(&literal), None);
    let empty_pattern = RenameRule {
        regex: true,
        ..Default::default()
    };
    assert_eq!(regex_error(&empty_pattern), None);
}

#[test]
fn invalid_regex_pattern_leaves_names_unchanged_instead_of_panicking() {
    let rule = RenameRule {
        find: "(unclosed".into(),
        replace: "x".into(),
        regex: true,
        ..Default::default()
    };
    let plans = plan_files(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
    assert_eq!(plans[0].to, "a.txt");
    assert_eq!(plans[0].status, PlanStatus::Unchanged);
}

#[test]
fn prefix_suffix_and_case_apply_to_the_stem_only() {
    let rule = RenameRule {
        prefix: "v_".into(),
        suffix: "_final".into(),
        case: CaseMode::Upper,
        ..Default::default()
    };
    let n = names(&["report.txt"]);
    let plans = plan_files(&n, &existing(&["report.txt"]), &rule);
    // stem REPORT -> v_REPORT_final, extension untouched.
    assert_eq!(plans[0].to, "v_REPORT_final.txt");
}

#[test]
fn numbering_uses_input_order_with_padding() {
    let rule = RenameRule {
        prefix: "shot_".into(),
        numbering: Some(Numbering {
            start: 5,
            step: 5,
            pad: 3,
        }),
        ..Default::default()
    };
    // Counter follows input order; it is appended to the stem, ahead of
    // the extension. The third name has no extension.
    let n = names(&["a.png", "b.png", "c"]);
    let plans = plan_files(&n, &existing(&[]), &rule);
    assert_eq!(plans[0].to, "shot_a005.png");
    assert_eq!(plans[1].to, "shot_b010.png");
    assert_eq!(plans[2].to, "shot_c015");
}

#[test]
fn dotfiles_keep_their_leading_dot_as_stem() {
    let rule = RenameRule {
        prefix: "x".into(),
        ..Default::default()
    };
    let plans = plan_files(&names(&[".gitignore"]), &existing(&[".gitignore"]), &rule);
    assert_eq!(plans[0].to, "x.gitignore");
}

#[test]
fn unchanged_when_rule_is_a_noop_for_the_name() {
    let rule = RenameRule {
        find: "zzz".into(),
        replace: "q".into(),
        ..Default::default()
    };
    let plans = plan_files(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
    assert_eq!(plans[0].status, PlanStatus::Unchanged);
    assert_eq!(plans[0].to, "a.txt");
}

#[test]
fn two_entries_to_same_target_are_collisions() {
    // Both lose their unique digits, colliding on "img.jpg".
    let rule = RenameRule {
        find: "1".into(),
        replace: "".into(),
        ..Default::default()
    };
    let n = names(&["img1.jpg", "img11.jpg"]);
    let plans = plan_files(
        &n,
        &existing(&n.iter().map(String::as_str).collect::<Vec<_>>()),
        &rule,
    );
    assert_eq!(plans[0].to, "img.jpg");
    assert_eq!(plans[1].to, "img.jpg");
    assert_eq!(plans[0].status, PlanStatus::Collision);
    assert_eq!(plans[1].status, PlanStatus::Collision);
    assert!(!plan_is_applicable(&plans));
}

#[test]
fn collision_with_an_untouched_sibling_is_flagged() {
    let rule = RenameRule {
        find: "draft".into(),
        replace: "final".into(),
        ..Default::default()
    };
    // "final.txt" already exists and is NOT in the rename batch.
    let n = names(&["draft.txt"]);
    let ex = existing(&["draft.txt", "final.txt"]);
    let plans = plan_files(&n, &ex, &rule);
    assert_eq!(plans[0].status, PlanStatus::Collision);
}

#[test]
fn vacate_guard_suppresses_sibling_check_not_duplicate_check() {
    // "a.txt" -> "b.txt"; the existing "b.txt" is a collision only when it
    // is NOT part of the batch. When "b.txt" is also selected (and so will
    // be vacated), the guard lets the rename through.
    let rule = RenameRule {
        find: "a".into(),
        replace: "b".into(),
        ..Default::default()
    };
    let ex = existing(&["a.txt", "b.txt"]);
    // b.txt not in batch -> collision with an untouched sibling.
    assert_eq!(
        plan_files(&names(&["a.txt"]), &ex, &rule)[0].status,
        PlanStatus::Collision
    );
    // b.txt also in the batch -> a.txt's target is no longer a sibling
    // clash. (b.txt has no 'a', so it stays put and is Unchanged.)
    let plans = plan_files(&names(&["a.txt", "b.txt"]), &ex, &rule);
    assert_eq!(plans[0].to, "b.txt");
    assert_eq!(plans[1].status, PlanStatus::Unchanged);
    // With b.txt staying, both want "b.txt" -> the duplicate-target rule
    // still flags a.txt. The vacate guard only suppresses the *sibling*
    // check, not genuine duplicates.
    assert_eq!(plans[0].status, PlanStatus::Collision);
}

#[test]
fn invalid_target_names_are_flagged() {
    let rule = RenameRule {
        find: "a".into(),
        replace: "/".into(),
        ..Default::default()
    };
    let plans = plan_files(&names(&["a"]), &existing(&["a"]), &rule);
    assert_eq!(plans[0].status, PlanStatus::Invalid);
    assert!(!plan_is_applicable(&plans));
}

#[test]
fn nul_and_overlong_target_names_are_flagged() {
    let nul = RenameRule {
        find: "a".into(),
        replace: "\0".into(),
        ..Default::default()
    };
    let plans = plan_files(&names(&["a"]), &existing(&["a"]), &nul);
    assert_eq!(plans[0].status, PlanStatus::Invalid);

    let overlong = RenameRule {
        prefix: "x".repeat(255),
        ..Default::default()
    };
    let plans = plan_files(&names(&["a"]), &existing(&["a"]), &overlong);
    assert_eq!(plans[0].status, PlanStatus::Invalid);
    assert!(!plan_is_applicable(&plans));

    let at_limit = RenameRule {
        prefix: "x".repeat(254),
        ..Default::default()
    };
    let plans = plan_files(&names(&["a"]), &existing(&["a"]), &at_limit);
    assert_eq!(plans[0].status, PlanStatus::Ok);
}

#[test]
fn normalization_equivalent_sibling_is_a_collision() {
    // A decomposed "café.txt" (e + U+0301) already exists untouched; a
    // rename producing the precomposed form would land on it on APFS.
    let decomposed = "cafe\u{301}.txt";
    let rule = RenameRule {
        find: "Q".into(),
        replace: "\u{e9}".into(),
        ..Default::default()
    };
    let plans = plan_files(
        &names(&["cafQ.txt"]),
        &existing(&["cafQ.txt", decomposed]),
        &rule,
    );
    assert_eq!(plans[0].to, "caf\u{e9}.txt");
    assert_eq!(plans[0].status, PlanStatus::Collision);
}

#[test]
fn case_only_rename_of_the_entry_itself_is_ok() {
    // `foo.txt` matches `Foo.txt` under the case-insensitive key, but the
    // clash is the entry being renamed away, not an untouched sibling.
    let rule = RenameRule {
        case: CaseMode::Lower,
        ..Default::default()
    };
    let plans = plan_files(&names(&["Foo.txt"]), &existing(&["Foo.txt"]), &rule);
    assert_eq!(plans[0].to, "foo.txt");
    assert_eq!(plans[0].status, PlanStatus::Ok);
    assert!(plan_is_applicable(&plans));
}

#[test]
fn applicable_requires_a_change_and_no_problems() {
    // All unchanged -> not applicable (nothing to do).
    let noop = plan_files(
        &names(&["a.txt"]),
        &existing(&["a.txt"]),
        &RenameRule::default(),
    );
    assert!(!plan_is_applicable(&noop));
    // One clean change -> applicable.
    let rule = RenameRule {
        prefix: "p_".into(),
        ..Default::default()
    };
    let ok = plan_files(&names(&["a.txt"]), &existing(&["a.txt"]), &rule);
    assert!(plan_is_applicable(&ok));
    assert_eq!(ok.iter().filter(|p| p.status == PlanStatus::Ok).count(), 1);
}
