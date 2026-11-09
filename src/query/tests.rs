use super::*;
use std::path::PathBuf;
use std::time::Duration;

fn entry(name: &str, is_dir: bool, size: u64, modified: Option<SystemTime>) -> FileEntry {
    let ext = PathBuf::from(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    FileEntry {
        symlink: None,
        permissions: None,
        allocated_size: None,
        hard_links: None,
        owner_uid: None,
        owner_gid: None,
        accessed: None,
        created: None,
        accessed_str: "Unavailable".into(),
        created_str: "Unavailable".into(),
        changed: None,
        changed_str: "Unavailable".into(),
        name: name.to_string(),
        name_lower: name.to_lowercase(),
        sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
        path: PathBuf::from(format!("/x/{name}")),
        identity: crate::panel::ListingIdentity::Unavailable,
        is_dir,
        size,
        extension: ext,
        modified,
        modified_str: "-".to_string(),
        size_str: crate::panel::format_size(size),
    }
}

fn q(predicates: Vec<Predicate>) -> Query {
    Query {
        predicates,
        mode: MatchMode::Exact,
    }
}

#[test]
fn hard_link_count_queries_preserve_integer_boundaries_modes_and_saved_definitions() {
    let now = SystemTime::now();
    for mode in MatchMode::ALL {
        for expression in [
            "links:2",
            "links:=0",
            "links:>1",
            "links:<2",
            "links:2..4",
            "links:>=18446744073709551615",
            "links:<=18446744073709551615",
        ] {
            let query = Query::parse(expression, mode).unwrap();
            let rendered = Query::parse(&query.to_expression(), mode).unwrap();
            assert_eq!(query.predicates, rendered.predicates);
            let saved = crate::smart_folder::Definition {
                name: "Hard-linked files".into(),
                root: "/records".into(),
                query: query.clone(),
            };
            assert_eq!(
                serde_json::from_str::<crate::smart_folder::Definition>(
                    &serde_json::to_string(&saved).unwrap()
                )
                .unwrap(),
                saved
            );
            let mut e = entry("sample", false, 0, None);
            for n in [0, 1, 2, 3, 4, u64::MAX] {
                e.hard_links = Some(n);
                let expected = match expression {
                    "links:2" => n == 2,
                    "links:=0" => n == 0,
                    "links:>1" => n > 1,
                    "links:<2" => n < 2,
                    "links:2..4" => (2..=4).contains(&n),
                    "links:>=18446744073709551615" => n == u64::MAX,
                    _ => true,
                };
                assert_eq!(
                    query.matches_metadata(&e, now),
                    expected,
                    "{mode:?}:{expression}:{n}"
                );
            }
            e.hard_links = None;
            assert!(!query.matches_metadata(&e, now));
            e.hard_links = Some(2);
            e.is_dir = true;
            assert!(!query.matches_metadata(&e, now));
        }
        for bad in [
            "",
            "-1",
            "+2",
            "1.0",
            "1KB",
            "１",
            "1..",
            "..2",
            "3..2",
            "1..2..3",
            "18446744073709551616",
            ">18446744073709551615",
            "<0",
            "!2",
            "==2",
        ] {
            assert!(
                Query::parse(&format!("links:{bad}"), mode).is_err(),
                "{bad}"
            );
        }
    }
}

#[test]
fn filesystem_identity_query_ids_roundtrip_and_reject_ambiguous_numbers() {
    for mode in MatchMode::ALL {
        let query =
            Query::parse("DEVICE:00042 inode:!18446744073709551615 device:0", mode).unwrap();
        assert_eq!(
            query.predicates,
            vec![
                Predicate::DeviceId {
                    id: 42,
                    exclude: false
                },
                Predicate::Inode {
                    id: u64::MAX,
                    exclude: true
                },
                Predicate::DeviceId {
                    id: 0,
                    exclude: false
                }
            ]
        );
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        let restored: Query =
            serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, query);
        for field in ["device", "inode"] {
            for value in [
                "",
                "!",
                "-1",
                "+1",
                "1.0",
                "1KB",
                "0x10",
                "18446744073709551616",
            ] {
                assert!(
                    Query::parse(&format!("{field}:{value}"), mode).is_err(),
                    "{field}:{value}"
                );
            }
        }
    }
}

#[test]
fn parent_filter_is_literal_immediate_and_roundtrips() {
    let now = SystemTime::now();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut file = entry("child.txt", false, 1, None);
        file.path = PathBuf::from(std::ffi::OsString::from_vec(b"/x/\xff/child.txt".to_vec()));
        let query = Query::parse("parent:/x/�", MatchMode::Exact).unwrap();
        assert!(
            !query.matches_metadata(&file, now),
            "lossy labels cannot alias a byte path"
        );
    }
    for mode in MatchMode::ALL {
        let expression = if cfg!(windows) {
            r#"PARENT:"C:/x/a [literal]" case:insensitive"#
        } else {
            r#"PARENT:"/x/a [literal]" case:insensitive"#
        };
        let query = Query::parse(expression, mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        let restored: Query =
            serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored, query);
        for (path, matches) in [
            ("/x/a [literal]/child.txt", true),
            ("/x/a [literal]/folder", true),
            ("/x/a [literal]/nested/child.txt", false),
            ("/x/a [literal]-other/child.txt", false),
            ("/x/A [literal]/child.txt", false),
            ("/x/a [literal]", false),
        ] {
            let mut file = entry("child.txt", false, 1, None);
            file.path = PathBuf::from(if cfg!(windows) {
                format!("C:{path}")
            } else {
                path.into()
            });
            assert_eq!(
                query.matches_metadata(&file, now),
                matches,
                "{mode:?}: {path}"
            );
        }
        for value in ["", "relative", "/x/../y", "/x/\0y"] {
            let query = Query {
                predicates: vec![Predicate::Parent(value.into())],
                mode,
            };
            assert!(query.validate().is_err(), "{value:?}");
            assert!(Query::parse(&query.to_expression(), mode).is_err());
        }
        let mut file = entry("child.txt", false, 0, None);
        file.path = PathBuf::from(if cfg!(windows) {
            "C:/child.txt"
        } else {
            "/child.txt"
        });
        assert!(
            Query::parse(
                if cfg!(windows) {
                    "parent:C:/"
                } else {
                    "parent:/"
                },
                mode
            )
            .unwrap()
            .matches_metadata(&file, now)
        );
        assert!(
            !Query::parse(
                if cfg!(windows) {
                    "parent:C:/ parent:C:/other"
                } else {
                    "parent:/ parent:/other"
                },
                mode
            )
            .unwrap()
            .matches_metadata(&file, now)
        );
    }
}

#[test]
fn allocated_query_grammar_roundtrips_bounds_and_rejects_invalid_sizes() {
    for mode in MatchMode::ALL {
        for expression in [
            "allocated:>1KB size:<=4MB",
            "allocated:=0",
            "allocated:1KB..2KB",
            "allocated:<=18446744073709551615B",
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
        assert_eq!(
            Query::parse("allocated:>1KB", mode).unwrap().predicates,
            vec![Predicate::MinAllocated(1025)]
        );
        for value in [
            "",
            "<0",
            ">18446744073709551615B",
            "-1",
            "4KB..1KB",
            "1ZZ",
            "18446744073709551616B",
        ] {
            assert!(
                Query::parse(&format!("allocated:{value}"), mode).is_err(),
                "{value}"
            );
        }
    }
}

#[test]
fn owner_id_queries_roundtrip_and_reject_ambiguous_ids() {
    for mode in MatchMode::ALL {
        let query = Query::parse("UID:000501 gid:!18446744073709551615 uid:0", mode).unwrap();
        assert_eq!(
            query.predicates,
            vec![
                Predicate::OwnerId {
                    id: 501,
                    exclude: false
                },
                Predicate::GroupId {
                    id: u64::MAX,
                    exclude: true
                },
                Predicate::OwnerId {
                    id: 0,
                    exclude: false
                }
            ]
        );
        assert_eq!(
            query.chips(),
            vec!["uid:501", "gid:!18446744073709551615", "uid:0"]
        );
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for value in [
            "",
            "!",
            "!!1",
            "-1",
            "+1",
            "1.5",
            "1e3",
            "0x10",
            "alice",
            "1:2",
            "１２",
            "18446744073709551616",
            "000000000000000000000",
        ] {
            for field in ["uid", "gid"] {
                assert!(
                    Query::parse(&format!("{field}:{value}"), mode).is_err(),
                    "{field}:{value}"
                );
            }
        }
    }
}

#[test]
fn permission_query_validates_all_bits_and_saved_semantics() {
    for (label, bit) in PERMISSION_BITS {
        assert_eq!(
            parse_permissions(&format!("{label}+")).unwrap().groups,
            vec![PermissionGroup {
                required: bit,
                forbidden: 0,
                inverted: false
            }]
        );
        assert_eq!(
            parse_permissions(&format!("{label}-")).unwrap().groups,
            vec![PermissionGroup {
                required: 0,
                forbidden: bit,
                inverted: false
            }]
        );
    }
    assert_eq!(
        parse_permissions("UR+uw-ux+gs+sb-").unwrap().groups,
        vec![PermissionGroup {
            required: 0o2500,
            forbidden: 0o1200,
            inverted: false
        }]
    );
    for mode in MatchMode::ALL {
        let query = Query::parse("perm:ur+uw-ux+gs+sb- type:regular", mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        assert!(!query.requires_content());
    }
    for value in [
        "", "ur", "ur=", "zz+", "r+", "755", "ur+ur-", "ur+||uw+", "!!ur+", "é+", "ur+ ",
    ] {
        assert!(parse_permissions(value).is_err(), "{value}");
        assert!(
            q(vec![Predicate::UnixPermissions(value.into())])
                .validate()
                .is_err(),
            "{value}"
        );
    }
    assert!(Query::parse("perm:", MatchMode::Exact).is_err());
    assert_eq!(
        parse_permissions("ux+ux+").unwrap(),
        parse_permissions("ux+").unwrap()
    );
}

#[test]
fn advanced_permission_query_masks_groups_precedence_and_limits() {
    for mode in MatchMode::ALL {
        for expression in [
            "perm:uw+|gw+|ow+",
            "perm:!700+",
            "perm:us+|gs+ perm:uw-",
            "perm:0750+002-",
            "perm:!500+|us+",
            "perm:0+",
            "perm:!0+",
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
    }
    for (octal, symbolic) in [
        ("0750+", "ur+uw+ux+gr+gx+"),
        ("222-", "uw-gw-ow-"),
        ("700+|2000+", "ur+uw+ux+|gs+"),
        ("!500+002-|us+", "!ur+ux+ow-|us+"),
        ("7777+", "ur+uw+ux+gr+gw+gx+or+ow+ox+us+gs+sb+"),
    ] {
        let octal = parse_permissions(octal).unwrap();
        let symbolic = parse_permissions(symbolic).unwrap();
        for mode in 0..=0o7777 {
            assert_eq!(octal.matches(mode), symbolic.matches(mode), "{mode:o}");
        }
    }
    let any_write = parse_permissions("uw+|gw+|ow+").unwrap();
    let not_all = parse_permissions("!700+").unwrap();
    let inverted_group = parse_permissions("!500+002-|us+").unwrap();
    for mode in 0..=0o7777 {
        assert_eq!(any_write.matches(mode), mode & 0o222 != 0);
        assert_eq!(not_all.matches(mode), mode & 0o700 != 0o700);
        assert_eq!(
            inverted_group.matches(mode),
            !(mode & 0o500 == 0o500 && mode & 0o002 == 0) || mode & 0o4000 != 0
        );
        assert!(parse_permissions("0+").unwrap().matches(mode));
        assert!(!parse_permissions("!0+").unwrap().matches(mode));
    }
    for invalid in [
        "|ur+",
        "ur+|",
        "ur+||uw+",
        "!",
        "!!ur+",
        "ur+!uw-",
        "(ur+)",
        "888+",
        "10000+",
        "-700+",
        "0o755+",
        "7777+us-",
        "00000+",
        "750",
        "ur+||!",
        "ur+|uw+uw-",
    ] {
        assert!(parse_permissions(invalid).is_err(), "{invalid}");
    }
    assert!(parse_permissions(&vec!["ur+"; 64].join("|")).is_ok());
    assert!(parse_permissions(&vec!["ur+"; 65].join("|")).is_err());
    assert!(parse_permissions(&"ur+".repeat(1365)).is_ok());
    assert!(parse_permissions(&"ur+".repeat(1366)).is_err());
    assert!(parse_permissions(&"0+".repeat(2048)).is_ok());
    assert!(parse_permissions(&format!("{}|", "0+".repeat(2048))).is_err());
}

#[test]
fn regular_file_query_preserves_saved_semantics_and_unknown_kind() {
    for mode in MatchMode::ALL {
        for expression in ["type:regular", "TYPE:REGULAR-FILE", "type:regular ext:txt"] {
            let query = Query::parse(expression, mode).unwrap();
            assert!(query.predicates.contains(&Predicate::RegularFiles));
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            assert!(!query.matches(
                &entry("ordinary-looking.txt", false, 1, None),
                SystemTime::now()
            ));
        }
    }
    assert_eq!(parsed("type:file"), vec![Predicate::FilesOnly]);
}

#[test]
fn precise_age_query_preserves_strict_subsecond_boundaries_and_saved_semantics() {
    let now = std::time::UNIX_EPOCH + Duration::from_secs(10000);
    for mode in MatchMode::ALL {
        for (operator, expected) in [
            ("<", [true, false, false]),
            ("<=", [true, true, false]),
            ("=", [false, true, false]),
            (">=", [false, true, true]),
            (">", [false, false, true]),
        ] {
            let query = Query::parse(&format!("age:{operator}30min"), mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            for (age, expected) in [
                Duration::from_secs(1800) - Duration::from_nanos(1),
                Duration::from_secs(1800),
                Duration::from_secs(1800) + Duration::from_nanos(1),
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(
                    query.matches(&entry("a", false, 1, Some(now - age)), now),
                    expected,
                    "{operator}: {age:?}"
                );
            }
            assert!(!query.matches(&entry("a", false, 1, None), now));
        }
        let zero = Query::parse("age:=0s", mode).unwrap();
        assert!(zero.matches(
            &entry("future", false, 1, Some(now + Duration::from_secs(3600))),
            now
        ));
        assert_eq!(
            Query::parse("age:30m", mode).unwrap().to_expression(),
            "age:<=1800s"
        );
    }
    assert_eq!(parsed("date:<2m"), vec![Predicate::MaxAgeDays(60)]);
    assert_eq!(
        parsed("age:2m"),
        vec![Predicate::AgeSeconds {
            comparison: AgeComparison::AtMost,
            seconds: 120
        }]
    );
    for value in ["2h", "3d", "4w", "5SECONDS", "30min"] {
        assert!(Query::parse(&format!("age:{value}"), MatchMode::Exact).is_ok());
    }
    assert!(Query::parse(&format!("age:{}s", u64::MAX), MatchMode::Exact).is_ok());
    for value in [
        "30",
        "-1h",
        "1.5h",
        "3months",
        "!=3h",
        "18446744073709551615w",
        "18446744073709551616s",
    ] {
        assert!(
            Query::parse(&format!("age:{value}"), MatchMode::Exact).is_err(),
            "{value}"
        );
    }
}

#[test]
fn executable_query_roundtrips_across_modes() {
    for mode in MatchMode::ALL {
        for expression in ["type:executable", "TYPE:EXEC", "type:executable ext:sh"] {
            let query = Query::parse(expression, mode).unwrap();
            assert!(query.predicates.contains(&Predicate::ExecutableFiles));
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            assert!(!query.requires_content());
            assert!(!query.matches(&entry("a.sh", false, 0, None), SystemTime::now()));
        }
    }
}

#[test]
fn broken_symlink_query_aliases_and_type_alternatives_roundtrip() {
    for mode in MatchMode::ALL {
        for expression in [
            "type:broken-link",
            "TYPE:BROKEN-SYMLINK",
            "type:broken-link depth:<=2",
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert!(query.predicates.contains(&Predicate::BrokenSymlinks));
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert!(!query.requires_content());
            let saved = serde_json::to_string(&query).unwrap();
            assert_eq!(serde_json::from_str::<Query>(&saved).unwrap(), query);
        }
        let query = Query::parse("types:broken-symlink,regular", mode).unwrap();
        assert!(query.predicates.contains(&Predicate::TypesAny(vec![
            TypeCondition::BrokenSymlink,
            TypeCondition::Regular
        ])));
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
    }
    assert!(Query::parse("type:broken", MatchMode::Exact).is_err());
}

#[test]
fn symlink_query_roundtrips_across_modes() {
    for mode in MatchMode::ALL {
        for expression in ["type:symlink", "TYPE:LINK", "type:symlink ext:txt"] {
            let query = Query::parse(expression, mode).unwrap();
            assert!(query.predicates.contains(&Predicate::SymlinksOnly));
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            assert!(!query.requires_content());
            assert!(!query.matches(&entry("link.txt", false, 0, None), SystemTime::now()));
        }
    }
}

#[test]
fn accessed_boundaries_are_strict_precise_and_saved_query_safe() {
    let time = parse_absolute_time("2026-01-01T00:00:00.5Z").unwrap();
    let mut e = entry("file.txt", false, 1, Some(time));
    for mode in MatchMode::ALL {
        let q = Query::parse(
            "Accessed-after:2026-01-01T03:00:00.25+03:00 accessed-before:2026-01-01T00:00:00.75Z",
            mode,
        )
        .unwrap();
        assert!(!q.matches(&e, SystemTime::now()));
        e.accessed = Some(time);
        assert!(q.matches(&e, SystemTime::now()));
        assert_eq!(Query::parse(&q.to_expression(), mode).unwrap(), q);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&q).unwrap()).unwrap(),
            q
        );
        for field in ["accessed-after", "accessed-before"] {
            assert!(
                !Query::parse(&format!("{field}:2026-01-01T00:00:00.5Z"), mode)
                    .unwrap()
                    .matches(&e, SystemTime::now())
            );
        }
        e.accessed = None;
        for value in [
            "bad",
            "2026-13-01",
            "2026-01-01T00:00:00",
            "2026-01-01T25:00:00Z",
        ] {
            assert!(Query::parse(&format!("accessed-after:{value}"), mode).is_err());
            assert!(
                Query {
                    mode,
                    predicates: vec![Predicate::AccessedBefore(value.into())]
                }
                .validate()
                .is_err()
            );
        }
    }
    e.accessed = Some(parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap());
    assert!(
        Query::parse("accessed-before:1970-01-01", MatchMode::Exact)
            .unwrap()
            .matches(&e, SystemTime::now())
    );
}

#[test]
fn created_boundaries_are_strict_precise_and_saved_query_safe() {
    let time = parse_absolute_time("2026-01-01T00:00:00.5Z").unwrap();
    let mut e = entry("file.txt", false, 1, Some(time));
    for mode in MatchMode::ALL {
        let q = Query::parse(
            "Created-after:2026-01-01T03:00:00.25+03:00 created-before:2026-01-01T00:00:00.75Z",
            mode,
        )
        .unwrap();
        assert!(!q.matches(&e, SystemTime::now()));
        e.created = Some(time);
        assert!(q.matches(&e, SystemTime::now()));
        assert_eq!(Query::parse(&q.to_expression(), mode).unwrap(), q);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&q).unwrap()).unwrap(),
            q
        );
        for field in ["created-after", "created-before"] {
            assert!(
                !Query::parse(&format!("{field}:2026-01-01T00:00:00.5Z"), mode)
                    .unwrap()
                    .matches(&e, SystemTime::now())
            );
        }
        e.created = None;
        for value in [
            "bad",
            "2026-13-01",
            "2026-01-01T00:00:00",
            "2026-01-01T25:00:00Z",
        ] {
            assert!(Query::parse(&format!("created-after:{value}"), mode).is_err());
            assert!(
                Query {
                    mode,
                    predicates: vec![Predicate::CreatedBefore(value.into())]
                }
                .validate()
                .is_err()
            );
        }
    }
    e.created = Some(parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap());
    assert!(
        Query::parse("created-before:1970-01-01", MatchMode::Exact)
            .unwrap()
            .matches(&e, SystemTime::now())
    );
}

#[test]
fn changed_boundaries_are_strict_precise_and_saved_query_safe() {
    let time = parse_absolute_time("2026-01-01T00:00:00.5Z").unwrap();
    let mut e = entry("file.txt", false, 1, Some(time));
    for mode in MatchMode::ALL {
        let q = Query::parse(
            "Changed-after:2026-01-01T03:00:00.25+03:00 changed-before:2026-01-01T00:00:00.75Z",
            mode,
        )
        .unwrap();
        assert!(!q.matches(&e, SystemTime::now()));
        e.changed = Some(time);
        assert!(q.matches(&e, SystemTime::now()));
        assert_eq!(Query::parse(&q.to_expression(), mode).unwrap(), q);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&q).unwrap()).unwrap(),
            q
        );
        for field in ["changed-after", "changed-before"] {
            assert!(
                !Query::parse(&format!("{field}:2026-01-01T00:00:00.5Z"), mode)
                    .unwrap()
                    .matches(&e, SystemTime::now())
            );
        }
        e.changed = None;
        for value in [
            "bad",
            "2026-13-01",
            "2026-01-01T00:00:00",
            "2026-01-01T25:00:00Z",
        ] {
            assert!(Query::parse(&format!("changed-after:{value}"), mode).is_err());
            assert!(
                Query {
                    mode,
                    predicates: vec![Predicate::ChangedBefore(value.into())]
                }
                .validate()
                .is_err()
            );
        }
    }
    e.changed = Some(parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap());
    assert!(
        Query::parse("changed-before:1970-01-01", MatchMode::Exact)
            .unwrap()
            .matches(&e, SystemTime::now())
    );
}

#[test]
fn absolute_modified_dates_roundtrip_and_obey_strict_instant_boundaries() {
    let boundary = parse_absolute_time("2026-10-28").unwrap();
    assert_eq!(
        parse_absolute_time("2026-10-28T03:00:00+03:00").unwrap(),
        boundary
    );
    assert_eq!(
        parse_absolute_time("1969-12-31T23:59:59.5Z").unwrap(),
        std::time::UNIX_EPOCH - Duration::from_millis(500)
    );
    for mode in MatchMode::ALL {
        let query = Query::parse("After:2026-10-28 before:2026-10-29", mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for (modified, expected) in [
            (None, false),
            (Some(boundary), false),
            (boundary.checked_sub(Duration::from_nanos(1)), false),
            (boundary.checked_add(Duration::from_nanos(1)), true),
            (boundary.checked_add(Duration::from_secs(86400)), false),
        ] {
            assert_eq!(
                query.matches(&entry("a.rs", false, 1, modified), boundary),
                expected
            );
        }
        let fractional =
            Query::parse("after:1969-12-31T23:59:59.5Z before:1970-01-01", mode).unwrap();
        assert!(fractional.matches(
            &entry(
                "old",
                false,
                1,
                Some(std::time::UNIX_EPOCH - Duration::from_millis(250))
            ),
            boundary
        ));
    }
    for value in [
        "2026-02-30",
        "2026-1-1",
        "today",
        "2026-10-28T12:00:00",
        "2016-12-31T23:59:60Z",
    ] {
        assert!(
            Query::parse(&format!("after:{value}"), MatchMode::Exact).is_err(),
            "{value}"
        );
        assert!(
            q(vec![Predicate::ModifiedBefore(value.to_string())])
                .validate()
                .is_err()
        );
    }
}

#[test]
fn extension_alternatives_preserve_literal_boundaries_case_and_legacy_queries() {
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"exts:".RS, toml, tar.gz""#, mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for (name, directory, expected) in [
            ("main.rs", false, true),
            ("Cargo.toml", false, true),
            ("backup.tar.gz", false, true),
            ("backup.xtar.gz", false, false),
            ("folder.rs", true, false),
            (".rs", false, true),
            ("rs", false, false),
            ("main.rs.old", false, false),
        ] {
            assert_eq!(
                query.matches(&entry(name, directory, 1, None), SystemTime::UNIX_EPOCH),
                expected,
                "{name}"
            );
        }
        let query = Query::parse("case:sensitive exts:RS,TOML", mode).unwrap();
        assert!(query.matches(&entry("main.RS", false, 1, None), SystemTime::UNIX_EPOCH));
        assert!(!query.matches(&entry("main.rs", false, 1, None), SystemTime::UNIX_EPOCH));
    }
    let legacy = Query::parse("ext:rs,toml", MatchMode::Exact).unwrap();
    assert_eq!(
        Query::parse(&legacy.to_expression(), legacy.mode).unwrap(),
        legacy
    );
    assert!(legacy.matches(
        &entry("literal.rs,toml", false, 1, None),
        SystemTime::UNIX_EPOCH
    ));
    assert!(!legacy.matches(&entry("main.rs", false, 1, None), SystemTime::UNIX_EPOCH));
    for expression in [
        "exts:",
        "exts:rs,",
        "exts:,rs",
        "exts:rs,,toml",
        "exts:..rs",
        "exts:rs,*",
        "exts:rs,a/b",
    ] {
        assert!(
            Query::parse(expression, MatchMode::Exact).is_err(),
            "{expression}"
        );
    }
    for values in [
        vec![],
        vec!["rs,toml".into()],
        vec!["rs".into(), String::new()],
    ] {
        assert!(
            q(vec![Predicate::ExtensionsAny(values)])
                .validate()
                .is_err()
        );
    }
}

#[test]
fn negative_metadata_text_queries_preserve_modes_case_and_serialization() {
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"notname:"test draft" notpath:"vendor files""#, mode).unwrap();
        assert!(!query.requires_content());
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        let sensitive = Query::parse("case:sensitive notname:Draft", mode).unwrap();
        assert!(sensitive.matches(&entry("draft.txt", false, 1, None), SystemTime::UNIX_EPOCH));
        assert!(!sensitive.matches(&entry("Draft.txt", false, 1, None), SystemTime::UNIX_EPOCH));
    }
    for (mode, pattern, excluded, retained) in [
        (MatchMode::Exact, "draft", "old-draft.txt", "dr-a-ft.txt"),
        (MatchMode::Fuzzy, "drft", "dr-a-ft.txt", "done.txt"),
        (MatchMode::Regex, "^draft", "draft.txt", "old-draft.txt"),
    ] {
        let query = Query::parse(&format!("notname:{pattern}"), mode).unwrap();
        assert!(!query.matches(&entry(excluded, false, 1, None), SystemTime::UNIX_EPOCH));
        assert!(query.matches(&entry(retained, false, 1, None), SystemTime::UNIX_EPOCH));
    }
    for expression in ["notname:", "notpath:", "notname:[", "notpath:["] {
        assert!(Query::parse(expression, MatchMode::Regex).is_err());
    }
}

#[test]
fn case_sensitive_query_controls_text_masks_extensions_and_saved_filters() {
    for mode in MatchMode::ALL {
        for (expression, name, expected) in [
            ("case:sensitive name:Doc", "Doc.rs", true),
            ("case:sensitive name:Doc", "doc.rs", false),
            ("case:insensitive name:Doc", "doc.rs", true),
            ("case:sensitive ext:RS", "main.RS", true),
            ("case:sensitive ext:RS", "main.rs", false),
            ("case:sensitive glob:*.RS", "main.rs", false),
            ("case:sensitive exclude:*.RS", "main.rs", true),
            ("case:sensitive exclude:*.RS", "main.RS", false),
            (
                r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                "Readme.TXT",
                true,
            ),
            (
                r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                "readme.TXT",
                false,
            ),
            (
                r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                "Other",
                true,
            ),
            (
                r#"case:sensitive filter:"^Read && .TXT$ | 'Other""#,
                "other",
                false,
            ),
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            assert_eq!(
                query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                expected,
                "{mode:?}: {expression}: {name}"
            );
        }
    }
    for expression in ["case:", "case:automatic", "case:sensitive case:insensitive"] {
        assert!(Query::parse(expression, MatchMode::Exact).is_err());
    }
    assert!(
        q(vec![
            Predicate::CaseSensitive(true),
            Predicate::CaseSensitive(false)
        ])
        .validate()
        .is_err()
    );
}

#[test]
fn empty_query_type_parses_saves_and_activates_observation() {
    for mode in MatchMode::ALL {
        let query =
            Query::parse("case:smart name:lower type:EMPTY types:empty,folders", mode).unwrap();
        assert!(!query.requires_content());
        assert!(!query.case_sensitive());
        assert_eq!(query.predicates[2], Predicate::Empty);
        assert_eq!(query.chips()[2], "type:empty");
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
    }
}

#[test]
fn type_alternatives_parse_validate_and_preserve_conjunctions() {
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"case:smart name:lower types:"REGULAR-FILE, link, PIPE, char, block, dir, doc, exec""#, mode).unwrap();
        assert!(!query.requires_content());
        assert!(!query.case_sensitive());
        assert_eq!(
            query.chips()[2],
            "types:regular,symlink,fifo,char-device,block-device,folders,docs,executable"
        );
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        let query =
            Query::parse("types:fifo,socket types:regular,symlink type:file", mode).unwrap();
        assert_eq!(query.predicates.len(), 3);
        assert!(matches!(&query.predicates[0], Predicate::TypesAny(types) if types.len() == 2));
        assert!(matches!(&query.predicates[1], Predicate::TypesAny(types) if types.len() == 2));
        assert_eq!(
            Query::parse("type:fifo type:socket", mode)
                .unwrap()
                .predicates
                .len(),
            2
        );
        for expression in [
            "types:",
            "types:,file",
            "types:file,",
            "types:file,,socket",
            "types:file,unknown",
        ] {
            assert!(Query::parse(expression, mode).is_err(), "{expression}");
        }
        assert!(Query::parse(&format!("types:{}", vec!["fifo"; 64].join(",")), mode).is_ok());
        assert!(Query::parse(&format!("types:{}", vec!["fifo"; 65].join(",")), mode).is_err());
        assert!(
            Query {
                predicates: vec![Predicate::TypesAny(vec![])],
                mode
            }
            .validate()
            .is_err()
        );
        assert!(
            Query {
                predicates: vec![Predicate::TypesAny(vec![TypeCondition::File; 65])],
                mode
            }
            .validate()
            .is_err()
        );
    }
}

#[test]
fn special_file_query_aliases_round_trip_without_content_or_case_effects() {
    for mode in MatchMode::ALL {
        for (alias, kind) in [
            ("fifo", SpecialFileType::Fifo),
            ("pipe", SpecialFileType::Fifo),
            ("socket", SpecialFileType::Socket),
            ("block", SpecialFileType::BlockDevice),
            ("block-device", SpecialFileType::BlockDevice),
            ("char", SpecialFileType::CharDevice),
            ("char-device", SpecialFileType::CharDevice),
        ] {
            let query = Query::parse(
                &format!("case:smart name:lower type:{}", alias.to_uppercase()),
                mode,
            )
            .unwrap();
            assert!(!query.requires_content());
            assert!(!query.case_sensitive());
            assert_eq!(query.predicates[2], Predicate::SpecialFileType(kind));
            assert_eq!(query.chips()[2], format!("type:{}", kind.label()));
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
    }
}

#[test]
fn exact_name_query_is_literal_and_mode_independent() {
    for mode in MatchMode::ALL {
        for name in [
            "report[1]*?.txt",
            "two words.txt",
            "line\nname.txt",
            "ёж.文",
            " a ",
        ] {
            let query = Query {
                predicates: vec![Predicate::NameExact(name.into())],
                mode,
            };
            query.validate().unwrap();
            assert!(!query.requires_content());
            assert!(query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH));
            assert!(query.matches(&entry(name, true, 0, None), SystemTime::UNIX_EPOCH));
            for other in [
                format!("old-{name}"),
                format!("{name}.bak"),
                format!("{name}\n"),
            ] {
                assert!(!query.matches(&entry(&other, false, 1, None), SystemTime::UNIX_EPOCH));
            }
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            let negative = Query {
                predicates: vec![Predicate::NameNotExact(name.into())],
                mode,
            };
            negative.validate().unwrap();
            assert!(!negative.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH));
            assert!(negative.matches(
                &entry(&format!("old-{name}"), false, 1, None),
                SystemTime::UNIX_EPOCH
            ));
            assert_eq!(
                Query::parse(&negative.to_expression(), mode).unwrap(),
                negative
            );
        }
        assert!(Query::parse("nameexact:[", mode).is_ok());
        assert!(Query::parse("nameexact:", mode).is_err());
        assert!(Query::parse("notnameexact:", mode).is_err());
        assert!(
            Query {
                predicates: vec![Predicate::NameExact(String::new())],
                mode
            }
            .validate()
            .is_err()
        );
        assert!(
            Query {
                predicates: vec![Predicate::NameNotExact(String::new())],
                mode
            }
            .validate()
            .is_err()
        );
    }
}

#[test]
fn exact_name_query_case_policy_treats_regex_syntax_as_literal() {
    for mode in MatchMode::ALL {
        for (expression, candidate, expected) in [
            ("nameexact:Résumé.txt", "résumé.txt", true),
            ("case:sensitive nameexact:Résumé.txt", "résumé.txt", false),
            ("case:smart nameexact:Résumé.txt", "résumé.txt", false),
            ("case:smart nameexact:résumé.txt", "Résumé.txt", true),
            (r"case:smart nameexact:\S", r"\s", false),
            ("case:smart notnameexact:Report.txt", "report.txt", true),
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(
                query.matches(&entry(candidate, false, 1, None), SystemTime::UNIX_EPOCH),
                expected,
                "{mode:?}: {expression}"
            );
        }
    }
}

#[test]
fn any_name_query_groups_repeated_terms_without_changing_name_and() {
    for mode in MatchMode::ALL {
        let query = Query::parse(
            r#"anyname:error ext:txt anyname:"warning issued" notname:old"#,
            mode,
        )
        .unwrap();
        assert!(!query.requires_content());
        assert_eq!(
            query.predicates[0],
            Predicate::NameAny(vec!["error".into(), "warning issued".into()])
        );
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        let and = Query::parse("name:error name:warn", mode).unwrap();
        assert_eq!(and.predicates.len(), 2);
        assert!(
            !and.predicates
                .iter()
                .any(|p| matches!(p, Predicate::NameAny(_)))
        );
    }
    assert!(Query::parse("anyname:", MatchMode::Exact).is_err());
    assert!(Query::parse("anyname:valid anyname:[", MatchMode::Regex).is_err());
    assert!(q(vec![Predicate::NameAny(vec![])]).validate().is_err());
    assert!(
        q(vec![Predicate::NameAny(vec![String::new()])])
            .validate()
            .is_err()
    );
    assert!(
        q(vec![
            Predicate::NameAny(vec!["a".into()]),
            Predicate::NameAny(vec!["b".into()])
        ])
        .validate()
        .is_err()
    );
    assert!(Query::parse(&vec!["anyname:a"; 64].join(" "), MatchMode::Exact).is_ok());
    assert!(Query::parse(&vec!["anyname:a"; 65].join(" "), MatchMode::Exact).is_err());
    assert!(
        q(vec![Predicate::NameAny(vec!["a".repeat(64 * 1024)])])
            .validate()
            .is_ok()
    );
    assert!(
        q(vec![Predicate::NameAny(vec!["a".repeat(64 * 1024 + 1)])])
            .validate()
            .is_err()
    );
    assert!(
        q(vec![Predicate::NameAny(vec![
            "a".repeat(32 * 1024),
            "b".repeat(32 * 1024 + 1)
        ])])
        .validate()
        .is_err()
    );
}

#[test]
fn any_name_query_respects_modes_literals_and_independent_groups() {
    for (mode, expression, name, expected) in [
        (
            MatchMode::Exact,
            "anyname:invoice anyname:receipt",
            "invoice-2026.pdf",
            true,
        ),
        (
            MatchMode::Exact,
            "anyname:invoice anyname:receipt",
            "inv-oice.pdf",
            false,
        ),
        (
            MatchMode::Fuzzy,
            "anyname:invoice anyname:receipt",
            "inv-oice.pdf",
            true,
        ),
        (
            MatchMode::Regex,
            "anyname:^invoice anyname:^receipt",
            "old-invoice.pdf",
            false,
        ),
        (
            MatchMode::Regex,
            "anyname:^invoice anyname:^receipt",
            "receipt-2026.pdf",
            true,
        ),
        (
            MatchMode::Exact,
            r#"anyname:"a;b|c" anyname:"d,e""#,
            "b.txt",
            false,
        ),
        (
            MatchMode::Exact,
            r#"anyname:"a;b|c" anyname:"d,e""#,
            "a;b|c.txt",
            true,
        ),
        (
            MatchMode::Exact,
            "case:smart anyname:invoice anyname:RECEIPT",
            "Invoice.pdf",
            false,
        ),
        (
            MatchMode::Exact,
            "case:insensitive anyname:invoice anyname:RECEIPT",
            "Receipt.pdf",
            true,
        ),
    ] {
        let query = Query::parse(expression, mode).unwrap();
        assert_eq!(
            query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
            expected,
            "{mode:?}: {expression}: {name}"
        );
    }
    let query = Query::parse(
        "anyname:invoice content:ready anyname:receipt anycontent:error anycontent:warn",
        MatchMode::Exact,
    )
    .unwrap();
    assert!(matches!(&query.predicates[0], Predicate::NameAny(values) if values.len() == 2));
    assert!(matches!(&query.predicates[2], Predicate::ContentAny(values) if values.len() == 2));
    assert_eq!(query.chips()[0], "anyname:(\"invoice\" OR \"receipt\")");
    assert_eq!(
        Query::parse(&query.to_expression(), query.mode).unwrap(),
        query
    );
}

#[test]
fn any_name_query_smart_case_reads_all_member_literals() {
    for (values, sensitive) in [
        (vec![r"error\S*", "warn"], false),
        (vec![r"\p{Lu}"], true),
        (vec!["error", "WARN"], true),
        (vec![r"error\S*", r"warn\x{41}"], true),
    ] {
        let query = Query {
            predicates: vec![
                Predicate::SmartCase,
                Predicate::NameAny(values.into_iter().map(str::to_string).collect()),
            ],
            mode: MatchMode::Regex,
        };
        query.validate().unwrap();
        assert_eq!(query.case_sensitive(), sensitive);
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
    }
}

#[test]
fn any_content_query_groups_repeated_terms_without_changing_content_and() {
    for mode in MatchMode::ALL {
        let query = Query::parse(
            r#"anycontent:error ext:txt anycontent:"warning issued" content:ready"#,
            mode,
        )
        .unwrap();
        assert!(query.requires_content());
        assert_eq!(
            query.predicates[0],
            Predicate::ContentAny(vec!["error".into(), "warning issued".into()])
        );
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        let and = Query::parse("content:error content:warn", mode).unwrap();
        assert_eq!(and.predicates.len(), 2);
        assert!(
            !and.predicates
                .iter()
                .any(|p| matches!(p, Predicate::ContentAny(_)))
        );
    }
    assert!(Query::parse("anycontent:", MatchMode::Exact).is_err());
    assert!(Query::parse("anycontent:valid anycontent:[", MatchMode::Regex).is_err());
    assert!(q(vec![Predicate::ContentAny(vec![])]).validate().is_err());
    assert!(
        q(vec![Predicate::ContentAny(vec![String::new()])])
            .validate()
            .is_err()
    );
    assert!(
        q(vec![
            Predicate::ContentAny(vec!["a".into()]),
            Predicate::ContentAny(vec!["b".into()])
        ])
        .validate()
        .is_err()
    );
    assert!(Query::parse(&vec!["anycontent:a"; 64].join(" "), MatchMode::Exact).is_ok());
    assert!(Query::parse(&vec!["anycontent:a"; 65].join(" "), MatchMode::Exact).is_err());
    assert!(
        q(vec![Predicate::ContentAny(vec!["a".repeat(64 * 1024)])])
            .validate()
            .is_ok()
    );
    assert!(
        q(vec![Predicate::ContentAny(vec!["a".repeat(64 * 1024 + 1)])])
            .validate()
            .is_err()
    );
    assert!(
        q(vec![Predicate::ContentAny(vec![
            "a".repeat(32 * 1024),
            "b".repeat(32 * 1024 + 1)
        ])])
        .validate()
        .is_err()
    );
}

#[test]
fn any_content_query_smart_case_reads_all_member_literals() {
    for (values, sensitive) in [
        (vec![r"error\S*", "warn"], false),
        (vec![r"\p{Lu}"], true),
        (vec!["error", "WARN"], true),
        (vec![r"error\S*", r"warn\x{41}"], true),
    ] {
        let query = Query {
            predicates: vec![
                Predicate::SmartCase,
                Predicate::ContentAny(values.into_iter().map(str::to_string).collect()),
            ],
            mode: MatchMode::Regex,
        };
        query.validate().unwrap();
        assert_eq!(query.case_sensitive(), sensitive);
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
    }
}

#[test]
fn hidden_search_query_preserves_modes_aliases_conflicts_and_saved_conditions() {
    for mode in MatchMode::ALL {
        for expression in [
            "hidden:include",
            "hidden:exclude",
            "hidden:only",
            "hidden:ON",
            "hidden:off",
            "hidden:true hidden:include",
            "hidden:false hidden:exclude",
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
        for expression in [
            "hidden:",
            "hidden:maybe",
            "hidden:include hidden:only",
            "hidden:on hidden:off",
        ] {
            assert!(Query::parse(expression, mode).is_err());
        }
        for (condition, hidden, visible) in [
            ("hidden:include", true, true),
            ("hidden:exclude", false, true),
            ("hidden:only", true, false),
        ] {
            let query = Query::parse(condition, mode).unwrap();
            assert_eq!(
                query.matches(&entry(".env", false, 1, None), SystemTime::UNIX_EPOCH),
                hidden
            );
            assert_eq!(
                query.matches(&entry("plain.txt", false, 1, None), SystemTime::UNIX_EPOCH),
                visible
            );
        }
    }
    assert!(
        !Query::parse("case:smart hidden:ONLY content:ready", MatchMode::Exact)
            .unwrap()
            .case_sensitive()
    );
}

#[test]
fn smart_case_query_preserves_case_modes_structural_fields_and_saved_queries() {
    for mode in MatchMode::ALL {
        for (expression, sensitive) in [
            ("case:smart content:ready", false),
            ("case:SMART content:READY", true),
            ("case:smart contentword:Äpfel", true),
            ("case:smart contentline:готово", false),
            ("case:smart exts:rs,TXT", true),
            ("case:smart glob:*.txt", false),
            ("case:smart excludedir:Vendor/**", true),
            (
                "case:smart type:IMAGE perm:UR+ after:2026-01-01T00:00:00Z content:ready",
                false,
            ),
            ("case:smart case:smart content:ready", false),
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(query.case_sensitive(), sensitive, "{mode:?}: {expression}");
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
        for expression in [
            "case:smart case:sensitive",
            "case:smart case:insensitive",
            "case:sensitive case:smart",
        ] {
            assert!(Query::parse(expression, mode).is_err(), "{expression}");
        }
    }
    assert!(
        !Query::parse("content:READY", MatchMode::Exact)
            .unwrap()
            .case_sensitive()
    );
    assert!(
        !Query::parse("case:insensitive content:READY", MatchMode::Exact)
            .unwrap()
            .case_sensitive()
    );
}

#[test]
fn smart_case_query_regex_analysis_uses_literals_not_syntax_names() {
    for (pattern, sensitive) in [
        (r"ready\S*", false),
        (r"ready\p{Lu}", false),
        (r"\p{Lu}", true),
        (r"[a-z]+", false),
        (r"[A-Z]+", true),
        (r"[a-z&&[^B]]+", true),
        (r"[[:upper:]]", true),
        (r"(?P<UPPER>ready)", false),
        (r"\x{41}bc", true),
        (r"\x{61}bc", false),
        (r"(?i:READY)", true),
        ("(?x)# UPPER\nready", false),
        (r"ready\\S", true),
    ] {
        let query = Query {
            predicates: vec![
                Predicate::SmartCase,
                Predicate::ContentContains(pattern.into()),
            ],
            mode: MatchMode::Regex,
        };
        query.validate().unwrap();
        assert_eq!(query.case_sensitive(), sensitive, "{pattern}");
        assert_eq!(
            Query::parse(&query.to_expression(), MatchMode::Regex).unwrap(),
            query
        );
    }
    let literal = Query {
        predicates: vec![Predicate::SmartCase, Predicate::ContentWord(r"\S".into())],
        mode: MatchMode::Regex,
    };
    assert!(literal.case_sensitive());
    assert!(Query::parse("case:smart content:(", MatchMode::Regex).is_err());
}

#[test]
fn content_line_query_preserves_empty_literals_spaces_and_saved_conditions() {
    for mode in MatchMode::ALL {
        for expression in [
            "contentline:",
            "notcontentline:",
            "CONTENTLINE:",
            r#"contentline:" cat " notcontentline:["#,
            r#"contentline:"""#,
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert!(query.requires_content());
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
    }
    for value in ["one\ntwo", "one\rtwo", &"a".repeat(1 << 18)] {
        assert!(
            q(vec![Predicate::ContentLine(value.into())])
                .validate()
                .is_err()
        );
        assert!(
            q(vec![Predicate::ContentLineExcludes(value.into())])
                .validate()
                .is_err()
        );
    }
    for expression in ["content:", "contentword:", "name:", "notcontent:"] {
        assert!(Query::parse(expression, MatchMode::Exact).is_err());
    }
}

#[test]
fn content_word_query_preserves_literals_modes_and_saved_predicates() {
    for mode in MatchMode::ALL {
        for expression in [
            "contentword:cat",
            "notcontentword:cat",
            r#"contentword:"cat dog" notcontentword:["#,
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert!(query.requires_content());
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
    }
    for predicate in [
        Predicate::ContentWord(String::new()),
        Predicate::ContentWordExcludes(String::new()),
        Predicate::ContentWord("a".repeat(1 << 18)),
    ] {
        assert!(q(vec![predicate]).validate().is_err());
    }
    assert!(Query::parse("contentword:", MatchMode::Exact).is_err());
}

#[test]
fn negative_content_query_preserves_syntax_and_requires_inspection() {
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"content:allowed notcontent:"not expected""#, mode).unwrap();
        assert!(query.requires_content());
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        assert!(!query.matches(&entry("file.txt", false, 1, None), SystemTime::UNIX_EPOCH));
    }
    assert!(Query::parse("notcontent:[", MatchMode::Regex).is_err());
    assert!(Query::parse("notcontent:", MatchMode::Exact).is_err());
}

#[test]
fn archive_scope_query_round_trips_and_rejects_conflicting_controls() {
    for mode in MatchMode::ALL {
        for expression in [
            "archives:off",
            "archives:on",
            "archives:FALSE",
            "archives:TRUE",
            "archives:off archives:off",
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
    }
    for expression in ["archives:", "archives:maybe", "archives:on archives:off"] {
        assert!(Query::parse(expression, MatchMode::Exact).is_err());
    }
    assert!(
        q(vec![
            Predicate::SearchArchives(false),
            Predicate::SearchArchives(true)
        ])
        .validate()
        .is_err()
    );
}

#[test]
fn exact_size_file_query_excludes_folders_and_preserves_legacy_lower_bound() {
    for mode in MatchMode::ALL {
        for expression in ["type:file size:=0", "type:FILES size:=1KB"] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            let expected_size = if expression.ends_with('0') { 0 } else { 1024 };
            for (name, directory, size, expected) in [
                ("empty", false, expected_size, true),
                ("folder", true, expected_size, false),
                ("photo.jpg", false, expected_size, true),
                ("file", false, expected_size + 1, false),
            ] {
                assert_eq!(
                    query.matches(&entry(name, directory, size, None), SystemTime::UNIX_EPOCH),
                    expected,
                    "{expression}: {name}"
                );
            }
        }
    }
    assert_eq!(
        Query::parse("size:1KB", MatchMode::Exact)
            .unwrap()
            .predicates,
        [Predicate::MinSize(1024)]
    );
    for expression in ["size:=", "size:==1", "size:=-1", "size:=1unknown"] {
        assert!(
            Query::parse(expression, MatchMode::Exact).is_err(),
            "{expression}"
        );
    }
    for size in [9_007_199_254_740_993, u64::MAX] {
        let query = Query::parse(&format!("size:={size}B"), MatchMode::Exact).unwrap();
        assert_eq!(
            query.predicates,
            [Predicate::MinSize(size), Predicate::MaxSize(size)]
        );
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
        assert!(query.matches(&entry("exact", false, size, None), SystemTime::UNIX_EPOCH));
        assert!(!query.matches(
            &entry("below", false, size - 1, None),
            SystemTime::UNIX_EPOCH
        ));
    }
    assert!(Query::parse("size:=18446744073709551616B", MatchMode::Exact).is_err());
    assert!(Query::parse("size:=18446744073709551615KB", MatchMode::Exact).is_err());
}

#[cfg(unix)]
#[test]
fn absolute_directory_query_masks_keep_lexical_root_and_member_boundaries() {
    let root = Path::new("/scope");
    for (pattern, path, expected) in [
        ("/scope/vendor/cache", "/scope/vendor/cache", true),
        ("/scope/vendor/cache", "/scope/other/vendor/cache", false),
        ("/scope/vendor/*/cache", "/scope/vendor/a/cache", true),
        ("/scope/vendor/*/cache", "/scope/vendor/a/b/cache", false),
        ("/scope/vendor/**/cache", "/scope/vendor/cache", true),
        ("/scope/vendor/**/cache", "/scope/vendor/a/b/cache", true),
        ("/scope/vendor/**", "/scope/vendor", true),
        ("/scope/vendor/cache/", "/scope/vendor/cache", true),
        ("/scope/**", "/scope", false),
        ("/outside/cache", "/outside/cache", false),
    ] {
        assert_eq!(
            compile_directory_mask(pattern, false)
                .unwrap()
                .matches_directory(Path::new(path), root),
            expected,
            "{pattern}: {path}"
        );
    }
    assert!(
        !compile_directory_mask("/scope/**/cache", false)
            .unwrap()
            .matches_directory(Path::new("scope/vendor/cache"), Path::new(""))
    );
    assert!(
        !compile_directory_mask("/scope/CACHE", true)
            .unwrap()
            .matches_directory(Path::new("/scope/cache"), root)
    );
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"excludedir:"/scope/build cache/**""#, mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
    }
    for value in [
        "/",
        "//scope/cache",
        "/scope/../cache",
        "/scope/./cache",
        "/scope//cache",
        "/scope/**cache",
    ] {
        assert!(compile_directory_mask(value, false).is_err(), "{value}");
    }
}

#[test]
fn relative_directory_query_masks_are_component_aware_and_rooted() {
    let root = Path::new("/scope");
    for (pattern, path, expected) in [
        ("vendor/cache", "/scope/vendor/cache", true),
        ("vendor/cache", "/scope/other/vendor/cache", false),
        ("vendor/*/cache", "/scope/vendor/a/cache", true),
        ("vendor/*/cache", "/scope/vendor/a/b/cache", false),
        ("vendor/**/cache", "/scope/vendor/cache", true),
        ("vendor/**/cache", "/scope/vendor/a/b/cache", true),
        ("**/cache", "/scope/cache", true),
        ("**/cache", "/scope/a/b/cache", true),
        ("vendor/**", "/scope/vendor", true),
        ("vendor/**", "/scope/vendor/a/b", true),
        ("vendor/**/**/cache", "/scope/vendor/cache", true),
        ("./cache", "/scope/cache", true),
        ("./cache", "/scope/other/cache", false),
        ("vendor/cache/", "/scope/vendor/cache", true),
        ("vendor/cach?", "/scope/vendor/caché", true),
        ("vendor/[cache]", "/scope/vendor/cache", false),
        ("vendor/[cache]", "/scope/vendor/[cache]", true),
        ("vendor/*", "/scope/vendor/line\ncache", true),
        ("**/cache", "/outside/cache", false),
        ("./**", "/scope", false),
    ] {
        let mask = compile_directory_mask(pattern, false).unwrap();
        assert_eq!(
            mask.matches_directory(Path::new(path), root),
            expected,
            "{pattern}: {path}"
        );
    }
    let mask = compile_directory_mask("vendor/CACHE", true).unwrap();
    assert!(!mask.matches_directory(Path::new("/scope/vendor/cache"), root));
    assert!(
        compile_directory_mask("vendor/CACHE", false)
            .unwrap()
            .matches_directory(Path::new("/scope/vendor/cache"), root)
    );
    for mode in MatchMode::ALL {
        for expression in [
            "excludedir:vendor/cache",
            "excludedir:vendor/**/cache",
            "excludedir:./cache",
            r#"excludedir:"build cache/**/out""#,
        ] {
            let query = Query::parse(expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
    }
    for value in [
        "//cache",
        "./",
        "../cache",
        "vendor/../cache",
        "vendor/./cache",
        "vendor//cache",
        "vendor/cache//",
        "vendor/**cache",
        "vendor/c**ache",
    ] {
        assert!(compile_directory_mask(value, false).is_err(), "{value}");
    }
}

#[test]
fn directory_exclusion_query_preserves_files_case_and_saved_masks() {
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"excludedir:*.tmp excludedir:"build cache""#, mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for (name, directory, expected) in [
            ("cache.tmp", true, false),
            ("cache.TMP", true, false),
            ("cache.tmp", false, true),
            ("build cache", true, false),
            ("build cache", false, true),
            ("cache.tmp.txt", true, true),
        ] {
            assert_eq!(
                query.matches(&entry(name, directory, 1, None), SystemTime::UNIX_EPOCH),
                expected,
                "{mode:?}: {name}"
            );
        }
        let sensitive = Query::parse("case:sensitive excludedir:*.TMP", mode).unwrap();
        assert!(sensitive.matches(&entry("cache.tmp", true, 1, None), SystemTime::UNIX_EPOCH));
        assert!(!sensitive.matches(&entry("cache.TMP", true, 1, None), SystemTime::UNIX_EPOCH));
    }
    for value in ["", "target/../cache", &"ab".repeat(1 << 17)] {
        assert!(
            q(vec![Predicate::ExcludeDirGlob(value.into())])
                .validate()
                .is_err()
        );
    }
    assert!(Query::parse("excludedir:", MatchMode::Exact).is_err());
}

#[test]
fn exclude_file_query_preserves_masks_case_and_saved_semantics() {
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"excludefile:*.tmp excludefile:"draft ?.txt""#, mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for (name, directory, expected) in [
            ("data.tmp", false, false),
            ("data.TMP", false, false),
            ("data.tmp", true, true),
            ("data.tmp.txt", false, true),
            ("draft é.txt", false, false),
            ("draft ab.txt", false, true),
            ("line\n.tmp", false, false),
            ("keep.txt", false, true),
        ] {
            let hit = query.matches(&entry(name, directory, 1, None), SystemTime::UNIX_EPOCH);
            assert_eq!(hit, expected, "{mode:?}: {name}");
        }
        let query = Query::parse("case:sensitive excludefile:*.TMP", mode).unwrap();
        assert!(query.matches(&entry("data.tmp", false, 1, None), SystemTime::UNIX_EPOCH));
        assert!(!query.matches(&entry("data.TMP", false, 1, None), SystemTime::UNIX_EPOCH));
    }
    for value in ["", "dir/*.tmp", &"ab".repeat(1 << 17)] {
        assert!(
            q(vec![Predicate::ExcludeFileGlob(value.into())])
                .validate()
                .is_err()
        );
    }
    assert!(Query::parse("excludefile:", MatchMode::Exact).is_err());
    let literal = Query::parse("excludefile:[ab].tmp", MatchMode::Regex).unwrap();
    assert!(literal.matches(&entry("a.tmp", false, 1, None), SystemTime::UNIX_EPOCH));
    assert!(!literal.matches(&entry("[ab].tmp", false, 1, None), SystemTime::UNIX_EPOCH));
}

#[test]
fn exclusion_query_round_trips_and_rejects_invalid_masks() {
    for mode in MatchMode::ALL {
        let query = Query::parse(r#"exclude:*.bak exclude:"build cache""#, mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
    }
    for expression in ["exclude:", "exclude:target/*"] {
        assert!(Query::parse(expression, MatchMode::Exact).is_err());
    }
    assert!(
        q(vec![Predicate::ExcludeGlob(String::new())])
            .validate()
            .is_err()
    );
    assert!(
        q(vec![Predicate::ExcludeGlob("ab".repeat(1 << 17))])
            .validate()
            .is_err()
    );
}

#[test]
fn depth_query_parses_ranges_and_preserves_saved_conditions() {
    for expression in ["depth:<=1", "depth:>=2", "depth:0", "depth:1..3"] {
        let query = Query::parse(expression, MatchMode::Exact).unwrap();
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
    }
    assert_eq!(
        Query::parse("depth:2", MatchMode::Exact)
            .unwrap()
            .predicates,
        [Predicate::MinDepth(2), Predicate::MaxDepth(2)]
    );
    for expression in [
        "depth:",
        "depth:-1",
        "depth:+1",
        "depth:1.5",
        "depth:3..1",
        "depth:1..",
        "depth:<=",
        "depth:9999999999999999999999999",
    ] {
        assert!(
            Query::parse(expression, MatchMode::Exact).is_err(),
            "{expression}"
        );
    }
}

#[test]
fn glob_query_round_trips_and_uses_whole_unicode_names() {
    for mode in MatchMode::ALL {
        for (pattern, name, expected) in [
            ("*.RS", "main.rs", true),
            ("*.rs", "main.rs.bak", false),
            ("report", "myreport", false),
            ("report", "REPORT", true),
            ("caf?.txt", "café.txt", true),
            ("caf?.txt", "caf.txt", false),
            ("[x].txt", "[x].txt", true),
            ("[x].txt", "x.txt", false),
            ("report *.pdf", "report annual.pdf", true),
            ("*", "資料", true),
            ("?.txt", "İ.txt", true),
            ("*.txt", "line\nbreak.txt", true),
        ] {
            let query = q(vec![Predicate::NameGlob(pattern.into())]);
            let query = Query { mode, ..query };
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            let restored: Query =
                serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
            assert_eq!(
                restored.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                expected,
                "{pattern}: {name}"
            );
        }
    }
    assert!(
        q(vec![Predicate::NameGlob("ab".repeat(1 << 17))])
            .validate()
            .is_err()
    );
}

#[test]
fn extension_query_matches_literal_file_suffixes_in_every_mode() {
    for mode in [MatchMode::Exact, MatchMode::Fuzzy, MatchMode::Regex] {
        let query = Query::parse("ext:.TAR.GZ", mode).unwrap();
        assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
        let restored: Query =
            serde_json::from_str(&serde_json::to_string(&query).unwrap()).unwrap();
        for (name, dir, expected) in [
            ("backup.TAR.GZ", false, true),
            ("backup.tar.gz", true, false),
            ("backup.tar.gz.old", false, false),
            ("backup.xtar.gz", false, false),
            ("tar.gz", false, false),
            (".tar.gz", false, true),
        ] {
            assert_eq!(
                restored.matches(&entry(name, dir, 1, None), SystemTime::UNIX_EPOCH),
                expected,
                "{name}"
            );
        }
    }
    let query = q(vec![Predicate::Extension("RS".into())]);
    assert!(query.matches(&entry("main.rs", false, 1, None), SystemTime::UNIX_EPOCH));
    for expression in [
        "ext:",
        "ext:.",
        "ext:tar..gz",
        "ext:*.rs",
        "ext:a/b",
        "ext:tar.",
    ] {
        assert!(
            Query::parse(expression, MatchMode::Exact).is_err(),
            "{expression}"
        );
    }
    assert!(
        q(vec![Predicate::Extension(String::new())])
            .validate()
            .is_err()
    );
}

#[test]
fn conjunctive_filter_export_preserves_expression_and_search_results() {
    for pattern in [
        ".rs$ && !^test | .toml$",
        r#"^x && "a | b""#,
        r#""ab" && "bc""#,
        ".rs$ &&",
    ] {
        let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for name in ["main.rs", "test.rs", "test.toml", "x a | b", "abc"] {
            assert_eq!(
                query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                crate::name_filter::is_match(pattern, name),
                "{pattern:?}, {name:?}"
            );
        }
    }
}

#[test]
fn quoted_alternative_export_survives_expression_json_and_search_matching() {
    for pattern in [
        r#""a | b" | .rs$"#,
        r#".rs$ | "^a$""#,
        r#"" a ""#,
        r#".rs$ | "unfinished"#,
    ] {
        let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
        let parsed = Query::parse(&query.to_expression(), query.mode).unwrap();
        assert_eq!(parsed, query);
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for name in ["a | b", "main.rs", "a", "^a$", "x a y"] {
            assert_eq!(
                parsed.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                crate::name_filter::is_match(pattern, name)
            );
        }
    }
}

#[test]
fn alternative_pane_filters_roundtrip_and_use_the_shared_search_matcher() {
    for pattern in [
        ".rs$ | .toml$",
        "ab | cd",
        "^é | !backup",
        "'literal | pipe",
    ] {
        let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
        assert_eq!(
            serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
            query
        );
        for name in [
            "main.rs",
            "Cargo.toml",
            "a-b",
            "Éclair",
            "backup",
            "literal | pipe",
        ] {
            assert_eq!(
                query.matches(&entry(name, false, 1, None), SystemTime::UNIX_EPOCH),
                crate::name_filter::is_match(pattern, name),
                "{pattern:?}, {name:?}"
            );
        }
    }
}

#[test]
fn pane_filter_predicate_roundtrips_all_modes_independently_of_query_mode() {
    for pattern in ["ab", "'a b ", "^i.txt$", "σ$", "''", "^$", "a:b", r"^a\b$"] {
        let query = from_panel_filter(pattern, &crate::panel::FacetSet::default());
        assert_eq!(
            Query::parse(&query.to_expression(), query.mode).unwrap(),
            query
        );
        let json = serde_json::to_string(&query).unwrap();
        assert_eq!(serde_json::from_str::<Query>(&json).unwrap(), query);
    }
    let query = Query::parse(r#"filter:"^a.b$""#, MatchMode::Regex).unwrap();
    assert!(query.matches(&entry("a.b", false, 1, None), SystemTime::UNIX_EPOCH));
    assert!(!query.matches(&entry("axb", false, 1, None), SystemTime::UNIX_EPOCH));
}

#[test]
fn literal_panel_filter_exports_its_original_matching_semantics() {
    let query = from_panel_filter("'a b ", &crate::panel::FacetSet::default());
    assert_eq!(query.mode, MatchMode::Exact);
    assert_eq!(query.predicates, [Predicate::NameFilter("'a b ".into())]);
    assert_eq!(
        from_panel_filter("'", &crate::panel::FacetSet::default()).predicates,
        [Predicate::NameFilter("'".into())]
    );
}

#[test]
fn empty_query_matches_everything() {
    let now = SystemTime::now();
    assert!(q(vec![]).matches(&entry("a.txt", false, 1, None), now));
}

#[test]
fn metadata_predicates_are_anded() {
    let now = SystemTime::now();
    let query = q(vec![
        Predicate::NameContains("log".into()),
        Predicate::MinSize(500),
        Predicate::MaxSize(1000),
    ]);
    assert!(query.matches(&entry("server.log", false, 800, None), now));
    assert!(!query.matches(&entry("server.log", false, 100, None), now));
    assert!(!query.matches(&entry("readme.md", false, 800, None), now));
}

#[test]
fn kind_and_age_predicates_use_entry_metadata() {
    let now = SystemTime::now();
    let recent = now - Duration::from_secs(2 * SECONDS_PER_DAY);
    let old = now - Duration::from_secs(40 * SECONDS_PER_DAY);
    assert!(q(vec![Predicate::Kind(Kind::Image)]).matches(&entry("a.png", false, 1, None), now));
    assert!(q(vec![Predicate::MaxAgeDays(7)]).matches(&entry("a", false, 1, Some(recent)), now));
    assert!(q(vec![Predicate::MinAgeDays(30)]).matches(&entry("a", false, 1, Some(old)), now));
    assert!(!q(vec![Predicate::MaxAgeDays(7)]).matches(&entry("a", false, 1, None), now));
}

#[test]
fn grammar_parses_fields_quotes_ranges_and_units() {
    let query = Query::parse(
        "name:\"quarterly report\" path:Finance type:docs size:1MB..2GB date:<2w content:revenue",
        MatchMode::Exact,
    )
    .unwrap();
    assert_eq!(
        query.predicates,
        vec![
            Predicate::NameContains("quarterly report".into()),
            Predicate::PathContains("Finance".into()),
            Predicate::Kind(Kind::Document),
            Predicate::MinSize(1 << 20),
            Predicate::MaxSize(2_u64 << 30),
            Predicate::MaxAgeDays(14),
            Predicate::ContentContains("revenue".into()),
        ]
    );
    assert!(query.requires_content());
}

#[test]
fn grammar_rejects_unknown_fields_and_invalid_regex() {
    assert!(Query::parse("owner:me", MatchMode::Exact).is_err());
    assert!(Query::parse("name:[", MatchMode::Regex).is_err());
    assert!(Query::parse("path:\"unfinished", MatchMode::Exact).is_err());
}

#[test]
fn expression_round_trip_is_canonical() {
    let first = Query::parse(
        "\"annual report\" type:pdf size:>=4MB date:>30d",
        MatchMode::Fuzzy,
    )
    .unwrap();
    let expression = first.to_expression();
    let second = Query::parse(&expression, MatchMode::Fuzzy).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.chips().len(), 4);
}

#[test]
fn content_queries_never_claim_a_metadata_only_match() {
    let now = SystemTime::now();
    let query = q(vec![Predicate::ContentContains("needle".into())]);
    let file = entry("a.txt", false, 1, None);
    assert!(query.matches_metadata(&file, now));
    assert!(!query.matches(&file, now));
}

#[test]
fn fuzzy_and_regex_modes_apply_to_text_predicates() {
    let now = SystemTime::now();
    let file = entry("quarterly-report.txt", false, 1, None);
    let fuzzy = Query::parse("qrpt", MatchMode::Fuzzy).unwrap();
    let regex = Query::parse("^quarterly.*\\.txt$", MatchMode::Regex).unwrap();
    assert!(fuzzy.matches(&file, now));
    assert!(regex.matches(&file, now));
}

#[test]
fn tokenizer_preserves_regex_escapes() {
    let query = Query::parse(r"^quarterly.*\.txt$", MatchMode::Regex).unwrap();
    assert_eq!(
        query.predicates,
        vec![Predicate::NameContains(r"^quarterly.*\.txt$".into())]
    );
    assert!(query.matches(
        &entry("quarterly-report.txt", false, 1, None),
        SystemTime::now()
    ));
    assert!(!query.matches(
        &entry("quarterly-reportXtxt", false, 1, None),
        SystemTime::now()
    ));
}

fn round_trip(query: &Query) -> Query {
    Query::parse(&query.to_expression(), query.mode).unwrap()
}

#[test]
fn name_with_colon_survives_round_trip() {
    let query = from_panel_filter("12:30", &crate::panel::FacetSet::default());
    assert_eq!(round_trip(&query), query);
    let query = Query::parse("name:a:b", MatchMode::Exact).unwrap();
    assert_eq!(
        query.predicates,
        vec![Predicate::NameContains("a:b".into())]
    );
    assert_eq!(round_trip(&query), query);
    let spaced = q(vec![Predicate::NameContains("note: draft".into())]);
    assert_eq!(round_trip(&spaced), spaced);
}

#[test]
fn quoted_token_with_colon_is_name_text() {
    let query = Query::parse("\"note: draft\" 'a:b'", MatchMode::Exact).unwrap();
    assert_eq!(
        query.predicates,
        vec![
            Predicate::NameContains("note: draft".into()),
            Predicate::NameContains("a:b".into()),
        ]
    );
}

#[test]
fn apostrophe_value_survives_round_trip() {
    let query = q(vec![
        Predicate::NameContains("it's".into()),
        Predicate::PathContains("bob's files".into()),
    ]);
    assert_eq!(round_trip(&query), query);
}

#[test]
fn backslash_escapes_quotes_outside_quotes() {
    let query = Query::parse(r#"foo\"bar it\'s"#, MatchMode::Exact).unwrap();
    assert_eq!(
        query.predicates,
        vec![
            Predicate::NameContains("foo\"bar".into()),
            Predicate::NameContains("it's".into()),
        ]
    );
}

#[test]
fn doubled_regex_backslash_survives_expression_round_trip() {
    let query = Query {
        mode: MatchMode::Regex,
        predicates: vec![Predicate::NameContains(r"a\\d".into())],
    };

    assert_eq!(round_trip(&query), query);
    assert_eq!(query.to_expression(), r#""a\\\\d""#);
}

#[test]
fn future_mtime_counts_as_today() {
    let now = SystemTime::now();
    let future = now + Duration::from_secs(3 * SECONDS_PER_DAY);
    let file = entry("a", false, 1, Some(future));
    assert!(q(vec![Predicate::MaxAgeDays(1)]).matches(&file, now));
    assert!(!q(vec![Predicate::MinAgeDays(1)]).matches(&file, now));
}

#[test]
fn strict_size_decimal_bounds_round_exactly_without_float_saturation() {
    for mode in MatchMode::ALL {
        for (value, bytes) in [
            ("9007199254740993.0B", 9_007_199_254_740_993),
            ("18446744073709551614.4B", u64::MAX - 1),
            ("18446744073709551614.5B", u64::MAX),
            ("18446744073709551615.4B", u64::MAX),
            ("0.4999999999999999999999999999999999999B", 0),
            ("0.5B", 1),
            (".5KB", 512),
            ("1.B", 1),
            ("0.0004882812499999999999999999999999999KB", 0),
            ("0.00048828125KB", 1),
            ("0.0000000000000000000000000000000000001TB", 0),
        ] {
            let query = Query::parse(&format!("size:={value}"), mode).unwrap();
            assert_eq!(
                query.predicates,
                vec![Predicate::MinSize(bytes), Predicate::MaxSize(bytes)],
                "{value}"
            );
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
        }
        for value in [
            "18446744073709551615.5B",
            "18446744073709551616.0B",
            "18446744073709551615.0KB",
        ] {
            assert!(
                Query::parse(&format!("size:={value}"), mode).is_err(),
                "{value}"
            );
        }
    }
}

#[test]
fn strict_size_upper_bound_never_saturates_to_an_inclusive_match() {
    for mode in MatchMode::ALL {
        assert!(Query::parse(&format!("size:>{}B", u64::MAX), mode).is_err());
        for expression in [
            format!("size:>={}B", u64::MAX),
            format!("size:>{}B", u64::MAX - 1),
            format!("size:<{}B", u64::MAX),
        ] {
            let query = Query::parse(&expression, mode).unwrap();
            assert_eq!(Query::parse(&query.to_expression(), mode).unwrap(), query);
            assert_eq!(
                serde_json::from_str::<Query>(&serde_json::to_string(&query).unwrap()).unwrap(),
                query
            );
            let is_upper = !expression.starts_with("size:<");
            assert_eq!(
                query.matches(&entry("max", false, u64::MAX, None), SystemTime::UNIX_EPOCH),
                is_upper
            );
            assert_eq!(
                query.matches(
                    &entry("below", false, u64::MAX - 1, None),
                    SystemTime::UNIX_EPOCH
                ),
                !is_upper
            );
        }
    }
}

#[test]
fn strict_size_operators_exclude_the_bound() {
    let now = SystemTime::now();
    let positive = Query::parse("size:>0", MatchMode::Exact).unwrap();
    assert!(!positive.matches(&entry("empty", false, 0, None), now));
    assert!(positive.matches(&entry("one", false, 1, None), now));
    let small = Query::parse("size:<1KB", MatchMode::Exact).unwrap();
    assert!(!small.matches(&entry("k", false, 1024, None), now));
    assert!(small.matches(&entry("k", false, 1023, None), now));
    let inclusive = Query::parse("size:<=1KB", MatchMode::Exact).unwrap();
    assert!(inclusive.matches(&entry("k", false, 1024, None), now));
    assert!(Query::parse("size:<0", MatchMode::Exact).is_err());
}

fn parse_error(expression: &str) -> String {
    Query::parse(expression, MatchMode::Exact)
        .unwrap_err()
        .message
}

fn parsed(expression: &str) -> Vec<Predicate> {
    Query::parse(expression, MatchMode::Exact)
        .unwrap()
        .predicates
}

#[test]
fn field_names_are_case_insensitive_and_need_a_value() {
    assert_eq!(parsed("NAME:x"), vec![Predicate::NameContains("x".into())]);
    assert_eq!(parsed("Type:docs"), vec![Predicate::Kind(Kind::Document)]);
    assert_eq!(parse_error(":foo"), "unknown query field");
    assert_eq!(parse_error("name:"), "field value is empty");
}

#[test]
fn size_units_accept_fractions_and_binary_aliases() {
    assert_eq!(parsed("size:1.5k"), vec![Predicate::MinSize(1536)]);
    assert_eq!(parsed("size:2kib"), vec![Predicate::MinSize(2048)]);
    assert_eq!(parse_error("size:MB"), "invalid size");
    assert_eq!(parse_error("size:1.2.3K"), "invalid size");
}

#[test]
fn size_ranges_must_be_ordered_and_closed() {
    assert_eq!(parse_error("size:5MB..1MB"), "size range is reversed");
    // Open-ended ranges are not supported; the empty side is no number.
    assert_eq!(parse_error("size:1MB.."), "invalid size");
    assert_eq!(parse_error("size:..5MB"), "invalid size");
}

#[test]
fn date_units_reject_overflow_and_treat_m_as_months() {
    assert_eq!(
        parse_error("date:99999999999999999y"),
        "date age is too large"
    );
    assert_eq!(parse_error("date:3h"), "unknown date unit");
    // `m` means months (30 days), not minutes.
    assert_eq!(parsed("date:<2m"), vec![Predicate::MaxAgeDays(60)]);
}

#[test]
fn panel_filter_converts_to_smart_folder_query() {
    let facets = crate::panel::FacetSet {
        kind: Some(crate::panel::KindFacet::Images),
        min_size: Some(1 << 20),
        max_age_days: Some(7),
        min_age_days: Some(30),
    };
    let query = from_panel_filter(" raw ", &facets);
    assert_eq!(query.mode, MatchMode::Exact);
    assert_eq!(
        query.predicates,
        vec![
            Predicate::NameFilter("raw".into()),
            Predicate::Kind(Kind::Image),
            Predicate::MinSize(1 << 20),
            Predicate::MaxAgeDays(7),
            Predicate::MinAgeDays(30),
        ]
    );
}
