//! Bounded filtering of displayed internal archive paths; no member I/O.
/// Literal, case-insensitive filename suffixes; independent of path-query options.
#[derive(Debug)]
pub struct ExtensionFilter(Vec<String>);
impl ExtensionFilter {
    pub fn compile(input: &str) -> Result<Self, String> {
        let error = "Use up to 32 literal extensions separated by spaces, e.g. pdf jpg tar.gz. Optional leading dots are accepted; no wildcards or paths. Maximum 256 bytes.";
        if input.len() > 256 {
            return Err(error.into());
        }
        let mut suffixes = Vec::new();
        for token in input.split_whitespace() {
            if suffixes.len() == 32 {
                return Err(error.into());
            }
            let token = token.strip_prefix('.').unwrap_or(token);
            if token.split('.').any(str::is_empty)
                || token.chars().any(|c| {
                    c.is_control()
                        || matches!(
                            c,
                            '/' | '\\' | '*' | '?' | '[' | ']' | '{' | '}' | ',' | ';' | '"' | '\''
                        )
                })
            {
                return Err(error.into());
            }
            suffixes.push(format!(".{}", token.to_lowercase()));
        }
        suffixes.sort_unstable();
        suffixes.dedup();
        Ok(Self(suffixes))
    }

    /// `lower_path` is the listing owner's cached lowercase comparison path.
    pub fn matches(&self, lower_path: &str) -> bool {
        self.0.is_empty() || {
            let name = filename(lower_path);
            self.0.iter().any(|suffix| name.ends_with(suffix))
        }
    }
}

#[derive(Debug)]
pub struct ModifiedFilter {
    since: Option<std::time::SystemTime>,
    before: Option<std::time::SystemTime>,
}
impl ModifiedFilter {
    pub fn compile(input: &str) -> Result<Self, String> {
        let error = "Use UTC dates: 2026-01-01 (day), 2026-01-01.. (since), ..2026-02-01 (before), or a range. The ending date is excluded.";
        if input.len() > 24 {
            return Err(error.into());
        }
        let input = input.trim();
        if input.is_empty() {
            return Ok(Self {
                since: None,
                before: None,
            });
        }
        let date = |text: &str| -> Result<Option<chrono::NaiveDate>, String> {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            if text.len() != 10
                || !text.bytes().enumerate().all(|(i, b)| {
                    if i == 4 || i == 7 {
                        b == b'-'
                    } else {
                        b.is_ascii_digit()
                    }
                })
            {
                return Err(error.into());
            }
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map(Some)
                .map_err(|_| error.into())
        };
        let instant = |date: chrono::NaiveDate| -> Result<std::time::SystemTime, String> {
            let seconds = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
            let duration = std::time::Duration::from_secs(seconds.unsigned_abs());
            (if seconds >= 0 {
                std::time::UNIX_EPOCH.checked_add(duration)
            } else {
                std::time::UNIX_EPOCH.checked_sub(duration)
            })
            .ok_or_else(|| error.into())
        };
        let (since, before) = if let Some((a, b)) = input.split_once("..") {
            (
                date(a)?.map(instant).transpose()?,
                date(b)?.map(instant).transpose()?,
            )
        } else {
            let day = date(input)?.ok_or_else(|| error.to_string())?;
            (
                Some(instant(day)?),
                Some(instant(day.succ_opt().ok_or_else(|| error.to_string())?)?),
            )
        };
        if (since.is_none() && before.is_none()) || since.zip(before).is_some_and(|(a, b)| a >= b) {
            return Err(error.into());
        }
        Ok(Self { since, before })
    }
    pub fn matches(&self, modified: Option<std::time::SystemTime>) -> bool {
        if self.since.is_none() && self.before.is_none() {
            return true;
        }
        modified.is_some_and(|time| {
            self.since.is_none_or(|since| time >= since)
                && self.before.is_none_or(|before| time < before)
        })
    }
}

#[derive(Debug)]
pub struct DepthFilter {
    minimum: Option<u32>,
    maximum: Option<u32>,
}
impl DepthFilter {
    pub fn compile(input: &str) -> Result<Self, String> {
        let error = "Use a depth (2), minimum (2..), maximum (..3), or inclusive range (2..4). Root members have depth 1.";
        if input.len() > 24 {
            return Err(error.into());
        }
        let input = input.trim();
        if input.is_empty() {
            return Ok(Self {
                minimum: None,
                maximum: None,
            });
        }
        let number = |text: &str| -> Result<Option<u32>, String> {
            let text = text.trim();
            if text.is_empty() {
                return Ok(None);
            }
            if !text.bytes().all(|b| b.is_ascii_digit()) {
                return Err(error.into());
            }
            text.parse().map(Some).map_err(|_| error.to_string())
        };
        let (minimum, maximum) = if let Some((a, b)) = input.split_once("..") {
            (number(a)?, number(b)?)
        } else {
            let exact = number(input)?;
            (exact, exact)
        };
        if (minimum.is_none() && maximum.is_none())
            || minimum.zip(maximum).is_some_and(|(min, max)| min > max)
        {
            return Err(error.into());
        }
        Ok(Self { minimum, maximum })
    }
    pub fn matches(&self, path: &std::path::Path) -> bool {
        if self.minimum.is_none() && self.maximum.is_none() {
            return true;
        }
        let depth = path
            .components()
            .filter(|c| matches!(c, std::path::Component::Normal(_)))
            .count();
        self.minimum.is_none_or(|min| depth >= min as usize)
            && self.maximum.is_none_or(|max| depth <= max as usize)
    }
}

#[derive(Debug)]
pub struct SizeFilter(Vec<(i8, u64)>);
impl SizeFilter {
    pub fn compile(input: &str) -> Result<Self, String> {
        let error = "Use up to two sizes: +1mi (minimum), -10mi (maximum), or 0b (exact). Units: b, k/m/g/t, ki/mi/gi/ti.";
        if input.len() > 64 {
            return Err(error.into());
        }
        let mut constraints = Vec::new();
        for token in input.split_whitespace() {
            if constraints.len() == 2 {
                return Err(error.into());
            }
            let (op, text) = if let Some(text) = token.strip_prefix('+') {
                (1, text)
            } else if let Some(text) = token.strip_prefix('-') {
                (-1, text)
            } else {
                (0, token)
            };
            let digits = text.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return Err(error.into());
            }
            let value = text[..digits]
                .parse::<u64>()
                .map_err(|_| error.to_string())?;
            let factor = match text[digits..].to_ascii_lowercase().as_str() {
                "" | "b" => 1,
                "k" => 1_000,
                "m" => 1_000_000,
                "g" => 1_000_000_000,
                "t" => 1_000_000_000_000,
                "ki" => 1 << 10,
                "mi" => 1 << 20,
                "gi" => 1 << 30,
                "ti" => 1 << 40,
                _ => return Err(error.into()),
            };
            constraints.push((
                op,
                value.checked_mul(factor).ok_or_else(|| error.to_string())?,
            ));
        }
        Ok(Self(constraints))
    }
    pub fn matches(&self, member: &crate::archive::ArchiveMember) -> bool {
        self.0.is_empty()
            || (member.is_regular
                && !member.is_dir
                && !member.is_symlink
                && self.0.iter().all(|&(op, limit)| match op {
                    1 => member.size >= limit,
                    -1 => member.size <= limit,
                    _ => member.size == limit,
                }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kinds {
    pub hide_dotfiles: bool,
    pub executable_only: bool,
    pub files: bool,
    pub folders: bool,
    pub links: bool,
    pub other: bool,
}
impl Default for Kinds {
    fn default() -> Self {
        Self {
            hide_dotfiles: false,
            executable_only: false,
            files: true,
            folders: true,
            links: true,
            other: true,
        }
    }
}
impl Kinds {
    pub fn matches(self, member: &crate::archive::ArchiveMember) -> bool {
        if self.hide_dotfiles && hidden_path(&member.path) {
            return false;
        }
        if self.executable_only
            && (!member.is_regular
                || member.is_dir
                || member.is_symlink
                || member.is_executable != Some(true))
        {
            return false;
        }
        if member.is_symlink {
            self.links
        } else if member.is_dir {
            self.folders
        } else if member.is_regular {
            self.files
        } else {
            self.other
        }
    }
}

/// Archive-relative dot components only; no host metadata or link traversal.
pub fn hidden_path(path: &std::path::Path) -> bool {
    path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if name.as_encoded_bytes().starts_with(b"."))
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub fuzzy: bool,
    pub glob: bool,
    pub exact: bool,
    pub basename_only: bool,
    pub regex: bool,
    pub case_sensitive: bool,
    pub smart_case: bool,
}
pub enum Pattern {
    Fuzzy {
        query: String,
        case_sensitive: bool,
        basename_only: bool,
    },
    Glob {
        pattern: globset::GlobMatcher,
        basename_only: bool,
        case_sensitive: bool,
    },
    All,
    Literal {
        exact: bool,
        needle: String,
        case_sensitive: bool,
        basename_only: bool,
    },
    Regex {
        pattern: regex::Regex,
        basename_only: bool,
    },
}
impl Pattern {
    pub fn compile(query: &str, options: Options) -> Result<Self, String> {
        if u8::from(options.regex)
            + u8::from(options.exact)
            + u8::from(options.glob)
            + u8::from(options.fuzzy)
            > 1
        {
            return Err("Choose only one of Regex, Exact, Glob or Fuzzy matching.".into());
        }
        if query.chars().count() > 256 {
            return Err("Archive filters are limited to 256 characters.".into());
        }
        let query = if options.regex || options.exact || options.glob {
            query
        } else {
            query.trim()
        };
        if query.is_empty() {
            return Ok(Self::All);
        }
        let case_sensitive = crate::preview_find::search_case_sensitive(
            query,
            options.case_sensitive,
            options.smart_case,
        );
        if options.fuzzy {
            return Ok(Self::Fuzzy {
                query: query.to_owned(),
                case_sensitive,
                basename_only: options.basename_only,
            });
        }
        if options.glob {
            let query = if case_sensitive {
                std::borrow::Cow::Borrowed(query)
            } else {
                std::borrow::Cow::Owned(query.to_lowercase())
            };
            let glob = globset::GlobBuilder::new(&query)
                .literal_separator(true)
                .backslash_escape(true)
                .build()
                .map_err(|error| error.to_string())?;
            return Ok(Self::Glob {
                pattern: glob.compile_matcher(),
                basename_only: options.basename_only,
                case_sensitive,
            });
        }
        if options.regex {
            return crate::preview_find::compile_search_pattern(query, case_sensitive, true, false)
                .map(|pattern| Self::Regex {
                    pattern,
                    basename_only: options.basename_only,
                })
                .map_err(|error| error.to_string());
        }
        Ok(Self::Literal {
            exact: options.exact,
            needle: if case_sensitive {
                query.to_owned()
            } else {
                query.to_lowercase()
            },
            case_sensitive,
            basename_only: options.basename_only,
        })
    }
    pub fn matches(&self, path: &str, lowercase_path: &str) -> bool {
        match self {
            Self::Fuzzy {
                query,
                case_sensitive,
                basename_only,
            } => crate::fuzzy::is_match_case(
                query,
                if *basename_only { filename(path) } else { path },
                *case_sensitive,
            ),

            Self::Glob {
                pattern,
                basename_only,
                case_sensitive,
            } => {
                let path = if *case_sensitive {
                    path
                } else {
                    lowercase_path
                };
                pattern.is_match(if *basename_only { filename(path) } else { path })
            }
            Self::All => true,
            Self::Literal {
                exact,
                needle,
                case_sensitive,
                basename_only,
            } => {
                let path = if *case_sensitive {
                    path
                } else {
                    lowercase_path
                };
                let path = if *basename_only { filename(path) } else { path };
                if *exact {
                    path == needle
                } else {
                    path.contains(needle)
                }
            }
            Self::Regex {
                pattern,
                basename_only,
            } => pattern.is_match(if *basename_only { filename(path) } else { path }),
        }
    }
}

fn filename(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modified_filter_uses_utc_days_half_open_bounds_and_known_timestamps() {
        use std::time::{Duration, UNIX_EPOCH};
        let end = UNIX_EPOCH + Duration::from_secs(86400);
        let day = ModifiedFilter::compile("1970-01-01").unwrap();
        assert!(day.matches(Some(UNIX_EPOCH)));
        assert!(day.matches(Some(end - Duration::from_nanos(1))));
        assert!(!day.matches(Some(end)));
        assert!(!day.matches(None));
        assert!(ModifiedFilter::compile("").unwrap().matches(None));
        let before = ModifiedFilter::compile("..1970-01-01").unwrap();
        assert!(before.matches(Some(UNIX_EPOCH - Duration::from_nanos(1))));
        assert!(!before.matches(Some(UNIX_EPOCH)));
        let since = ModifiedFilter::compile("1970-01-02..").unwrap();
        assert!(since.matches(Some(end)));
        assert!(!since.matches(Some(end - Duration::from_nanos(1))));
        assert!(
            ModifiedFilter::compile("1969-12-31")
                .unwrap()
                .matches(Some(UNIX_EPOCH - Duration::from_secs(1)))
        );
        assert!(ModifiedFilter::compile("2024-02-29").is_ok());
        assert!(ModifiedFilter::compile("9999-12-31").is_ok());
        for text in [
            "2023-02-29",
            "2026-13-01",
            "2026-1-01",
            "..",
            "1970-01-02..1970-01-01",
            "1970-01-01..1970-01-01",
            "1d",
            "1970-01-01T00:00:00Z",
        ] {
            assert!(ModifiedFilter::compile(text).is_err(), "{text}");
        }
    }
    #[test]
    fn glob_filter_handles_paths_alternatives_classes_escaping_and_case() {
        let paths = [
            "src/main.rs",
            "src/deep/lib.rs",
            "src/main.txt",
            "other/main.rs",
            "src/ЁЖ.rs",
            "src/a[1].rs",
        ];
        let mut options = Options {
            glob: true,
            ..Default::default()
        };
        assert!(matching("*.rs", options, &paths).is_empty());
        assert_eq!(matching("src/**/*.rs", options, &paths), [0, 1, 4, 5]);
        assert_eq!(matching("src/*.{rs,txt}", options, &paths), [0, 2, 4, 5]);
        assert_eq!(matching(r"src/a\[1\].rs", options, &paths), [5]);
        options.basename_only = true;
        assert_eq!(matching("[ml]*.rs", options, &paths), [0, 1, 3]);
        assert_eq!(matching("ёж.*", options, &paths), [4]);
        options.smart_case = true;
        assert!(matching("Main.rs", options, &paths).is_empty());
        options.smart_case = false;
        assert_eq!(matching("Main.rs", options, &paths), [0, 3]);
        assert_eq!(matching("?.rs", options, &["é.rs", "x.rs"]), [1]);
        assert_eq!(matching("??.rs", options, &["é.rs", "x.rs"]), [0]);
        for text in ["[", "{a,b"] {
            assert!(Pattern::compile(text, options).is_err());
        }
        options.exact = true;
        assert!(Pattern::compile("*", options).is_err());
        options.exact = false;
        options.regex = true;
        assert!(Pattern::compile("", options).is_err());
    }
    #[test]
    fn exact_filter_preserves_spaces_and_metacharacters_and_respects_scope_and_case() {
        let paths = [
            "src/a[1].txt",
            "src/a1.txt",
            "src/A[1].txt",
            "src/ a[1].txt ",
            "src/prefix-a[1].txt",
        ];
        let mut options = Options {
            exact: true,
            basename_only: true,
            ..Default::default()
        };
        assert_eq!(matching("a[1].txt", options, &paths), [0, 2]);
        assert_eq!(matching(" a[1].txt ", options, &paths), [3]);
        options.case_sensitive = true;
        assert_eq!(matching("a[1].txt", options, &paths), [0]);
        options.basename_only = false;
        assert!(matching("a[1].txt", options, &paths).is_empty());
        assert_eq!(matching("src/a[1].txt", options, &paths), [0]);
        assert_eq!(matching("", options, &paths), [0, 1, 2, 3, 4]);
        options.regex = true;
        assert!(Pattern::compile("", options).is_err());
        assert!(Pattern::compile("a", options).is_err());
    }
    #[test]
    fn filename_scope_handles_anchors_case_unicode_and_directory_components() {
        let paths = [
            "src/Main.rs",
            "src/main.rs",
            "main.rs/nested.txt",
            "other/ёж.txt",
            "files/ЁЖ.txt",
            "src/folder/",
        ];
        let mut options = Options {
            basename_only: true,
            ..Default::default()
        };
        assert!(matching("src", options, &paths).is_empty());
        assert_eq!(matching("ЁЖ", options, &paths), [3, 4]);
        assert_eq!(matching("folder", options, &paths), [5]);
        options.regex = true;
        assert_eq!(matching(r"^main\.rs$", options, &paths), [0, 1]);
        options.case_sensitive = true;
        assert_eq!(matching(r"^main\.rs$", options, &paths), [1]);
        assert_eq!(matching(r"(?i)^main\.rs$", options, &paths), [0, 1]);
        options.basename_only = false;
        assert!(matching(r"^main\.rs$", options, &paths).is_empty());
        options.regex = false;
        assert_eq!(matching("src", options, &paths), [0, 1, 5]);
    }
    fn matching(query: &str, options: Options, paths: &[&str]) -> Vec<usize> {
        let pattern = Pattern::compile(query, options).unwrap();
        paths
            .iter()
            .enumerate()
            .filter_map(|(i, p)| pattern.matches(p, &p.to_lowercase()).then_some(i))
            .collect()
    }
    #[test]
    fn fuzzy_archive_patterns_use_unicode_subsequences_scope_case_and_bounded_modes() {
        let paths = [
            "src/main.rs",
            "src/map.rs",
            "docs/Ёжик.txt",
            "docs/ёжик.txt",
            "src/İ.txt",
            "src/a b.rs",
        ];
        let mut options = Options {
            fuzzy: true,
            ..Default::default()
        };
        assert_eq!(matching("mnrs", options, &paths), [0]);
        assert!(matching("mnrs", Options::default(), &paths).is_empty());
        assert_eq!(matching("smrs", options, &paths), [0, 1]);
        options.basename_only = true;
        assert!(matching("smrs", options, &paths).is_empty());
        assert_eq!(matching("ёж", options, &paths), [2, 3]);
        options.smart_case = true;
        assert_eq!(matching("Ёж", options, &paths), [2]);
        options.smart_case = false;
        options.case_sensitive = true;
        assert_eq!(matching("ёж", options, &paths), [3]);
        options.case_sensitive = false;
        assert_eq!(matching("i", options, &paths), [0, 4]);
        assert_eq!(matching("a b", options, &paths), [5]);
        assert_eq!(matching("  mnrs  ", options, &paths), [0]);
        assert_eq!(matching(" ", options, &paths), [0, 1, 2, 3, 4, 5]);
        assert!(Pattern::compile(&"x".repeat(257), options).is_err());
        for conflicting in [
            Options {
                regex: true,
                ..options
            },
            Options {
                exact: true,
                ..options
            },
            Options {
                glob: true,
                ..options
            },
        ] {
            assert!(Pattern::compile("", conflicting).is_err());
        }
    }

    #[test]
    fn literal_defaults_keep_whitespace_trimming_metacharacters_and_unicode_lowercasing() {
        let paths = [
            "SRC/Main.rs",
            "src/ёж.txt",
            "src/ЁЖ.txt",
            "[a].txt",
            "İ.txt",
        ];
        assert_eq!(matching(" SRC ", Options::default(), &paths), [0, 1, 2]);
        assert_eq!(matching("ёж", Options::default(), &paths), [1, 2]);
        assert_eq!(matching("[a]", Options::default(), &paths), [3]);
        assert_eq!(matching("i\u{307}", Options::default(), &paths), [4]);
        assert_eq!(matching(" ", Options::default(), &paths), [0, 1, 2, 3, 4]);
    }
    #[test]
    fn regex_paths_smart_case_manual_priority_and_inline_flags_keep_their_meaning() {
        let paths = [
            "src/main.rs",
            "src/Main.rs",
            "src/MAIN.rs",
            "src/main.txt",
            "src/ёж.rs",
            "src/Ёж.rs",
        ];
        let mut options = Options {
            regex: true,
            smart_case: true,
            ..Default::default()
        };
        assert_eq!(matching(r"^src/main\.rs$", options, &paths), [0, 1, 2]);
        assert_eq!(matching(r"^src/Main\.rs$", options, &paths), [1]);
        assert_eq!(matching(r"ёж\.rs$", options, &paths), [4, 5]);
        assert_eq!(matching(r"Ёж\.rs$", options, &paths), [5]);
        options.case_sensitive = true;
        assert_eq!(matching(r"^src/main\.rs$", options, &paths), [0]);
        assert_eq!(matching(r"(?i)^src/Main\.rs$", options, &paths), [0, 1, 2]);
    }
    #[test]
    fn invalid_patterns_limits_and_regex_whitespace_do_not_fall_back_to_all_rows() {
        let options = Options {
            regex: true,
            ..Default::default()
        };
        for text in [
            "[".to_string(),
            "x".repeat(257),
            format!("{}x{}", "(".repeat(40), ")".repeat(40)),
        ] {
            assert!(Pattern::compile(&text, options).is_err());
        }
        assert!(Pattern::compile(&"é".repeat(256), Options::default()).is_ok());
        assert_eq!(matching(" ", options, &["a b", "ab"]), [0]);
        assert_eq!(matching("", options, &["ab"]), [0]);
    }
}
