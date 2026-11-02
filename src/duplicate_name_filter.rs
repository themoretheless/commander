//! Basename-only include/exclude globs captured before duplicate hashing.
use std::path::Path;
#[derive(Clone, Default, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Rules {
    pub include: String,
    pub exclude: String,
    pub case: bool,
    pub grouping: crate::duplicate_name_group::Component,
    pub grouping_ignore_ascii_case: bool,
    pub extension: crate::duplicate_extension::Mode,
}
#[derive(Clone, Default, Debug)]
pub struct Filter {
    pub rules: Rules,
    include: Option<globset::GlobSet>,
    exclude: Option<globset::GlobSet>,
}
#[derive(Default)]
pub struct Validation {
    cached: Option<(Rules, Result<Filter, String>)>,
}
impl Validation {
    pub fn get(&mut self, rules: &Rules) -> Result<Filter, String> {
        if self
            .cached
            .as_ref()
            .is_none_or(|(previous, _)| previous != rules)
        {
            self.cached = Some((rules.clone(), rules.compile()));
        }
        self.cached.as_ref().expect("validated above").1.clone()
    }
}
impl Rules {
    pub fn compile(&self) -> Result<Filter, String> {
        if self.include.len().saturating_add(self.exclude.len()) > 4096 {
            return Err("Name filters are limited to 4096 bytes in total".into());
        }
        let mut count = 0;
        let mut build = |raw: &str| -> Result<Option<globset::GlobSet>, String> {
            let mut builder = globset::GlobSetBuilder::new();
            let mut used = false;
            for pattern in raw.lines().map(str::trim).filter(|s| !s.is_empty()) {
                count += 1;
                if count > 64 {
                    return Err("Use at most 64 name patterns in total".into());
                }
                if pattern.contains('/') {
                    return Err("Name patterns match basenames; omit folder separators".into());
                }
                let glob = globset::GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .case_insensitive(!self.case)
                    .build()
                    .map_err(|e| format!("Invalid name pattern {pattern:?}: {e}"))?;
                builder.add(glob);
                used = true;
            }
            if used {
                builder.build().map(Some).map_err(|e| e.to_string())
            } else {
                Ok(None)
            }
        };
        let include = build(&self.include)?;
        let exclude = build(&self.exclude)?;
        Ok(Filter {
            rules: self.clone(),
            include,
            exclude,
        })
    }
}
impl Filter {
    pub fn matches(&self, path: &Path) -> bool {
        let Some(name) = path.file_name() else {
            return false;
        };
        let name = Path::new(name);
        self.include.as_ref().is_none_or(|set| set.is_match(name))
            && self.exclude.as_ref().is_none_or(|set| !set.is_match(name))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn include_union_exclusion_priority_case_and_basename_identity() {
        let rules = Rules {
            include: "*.{jpg,png}\nreport-?.txt".into(),
            exclude: "skip*\nreport-[ab].txt".into(),
            case: false,
            extension: Default::default(),
            ..Default::default()
        };
        let filter = rules.compile().unwrap();
        for name in [
            "/skip-folder/photo.JPG",
            "/nested/photo.png",
            "report-z.txt",
        ] {
            assert!(filter.matches(Path::new(name)), "{name}");
        }
        for name in ["skip.jpg", "report-a.txt", "report-zz.txt", "photo.txt"] {
            assert!(!filter.matches(Path::new(name)), "{name}");
        }
        let sensitive = Rules {
            case: true,
            ..rules
        }
        .compile()
        .unwrap();
        assert!(!sensitive.matches(Path::new("photo.JPG")));
        assert!(Filter::default().matches(Path::new(".hidden")));
        assert!(
            !Rules {
                exclude: "*".into(),
                ..Default::default()
            }
            .compile()
            .unwrap()
            .matches(Path::new("anything"))
        );
    }
    #[test]
    fn malformed_and_oversized_patterns_are_errors_without_a_broadened_filter() {
        for include in [
            "[".into(),
            "nested/*.jpg".into(),
            "x".repeat(4097),
            "*.jpg\n".repeat(65),
        ] {
            assert!(
                Rules {
                    include,
                    ..Default::default()
                }
                .compile()
                .is_err()
            );
        }
        let filter = Rules {
            include: " \n\t".into(),
            ..Default::default()
        }
        .compile()
        .unwrap();
        assert!(filter.matches(Path::new("a")));
    }
}
