//! Permission patterns, name and content matchers, and directory masks.

use super::*;

pub(super) const PERMISSION_BITS: [(&str, u32); 12] = [
    ("ur", 0o400),
    ("uw", 0o200),
    ("ux", 0o100),
    ("gr", 0o040),
    ("gw", 0o020),
    ("gx", 0o010),
    ("or", 0o004),
    ("ow", 0o002),
    ("ox", 0o001),
    ("us", 0o4000),
    ("gs", 0o2000),
    ("sb", 0o1000),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PermissionGroup {
    pub(super) required: u32,
    pub(super) forbidden: u32,
    pub(super) inverted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionPattern {
    pub(super) groups: Vec<PermissionGroup>,
}

impl PermissionPattern {
    pub fn matches(&self, mode: u32) -> bool {
        self.groups.iter().any(|group| {
            let matches = mode & group.required == group.required && mode & group.forbidden == 0;
            matches != group.inverted
        })
    }
}

/// AND within groups, OR between groups; ! negates one whole group.
/// Octal masks require all + bits or forbid all - bits, like symbolic terms.
pub fn parse_permissions(value: &str) -> Result<PermissionPattern, QueryError> {
    let invalid = || {
        QueryError::new(
            value.to_string(),
            "perm expects signed symbolic/octal bits, optionally grouped with | and !",
        )
    };
    if value.is_empty() || value.len() > 4096 {
        return Err(invalid());
    }
    let mut groups = Vec::new();
    for group in value.split('|') {
        if groups.len() >= 64 {
            return Err(invalid());
        }
        let (inverted, terms) = group
            .strip_prefix('!')
            .map_or((false, group), |terms| (true, terms));
        if terms.is_empty() {
            return Err(invalid());
        }
        let bytes = terms.as_bytes();
        let (mut required, mut forbidden, mut offset) = (0, 0, 0);
        while offset < bytes.len() {
            let bit = if bytes[offset].is_ascii_digit() {
                let start = offset;
                while offset < bytes.len() && bytes[offset].is_ascii_digit() {
                    offset += 1;
                }
                let digits = &terms[start..offset];
                if !(1..=4).contains(&digits.len()) {
                    return Err(invalid());
                }
                u32::from_str_radix(digits, 8).map_err(|_| invalid())?
            } else {
                let label_bytes = bytes.get(offset..offset + 2).ok_or_else(invalid)?;
                let label = std::str::from_utf8(label_bytes).map_err(|_| invalid())?;
                offset += 2;
                PERMISSION_BITS
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(label))
                    .map(|(_, bit)| *bit)
                    .ok_or_else(invalid)?
            };
            match bytes.get(offset) {
                Some(b'+') => required |= bit,
                Some(b'-') => forbidden |= bit,
                _ => return Err(invalid()),
            }
            offset += 1;
        }
        if required & forbidden != 0 {
            return Err(QueryError::new(
                value.to_string(),
                "conflicting permission requirements",
            ));
        }
        groups.push(PermissionGroup {
            required,
            forbidden,
            inverted,
        });
    }
    Ok(PermissionPattern { groups })
}

#[derive(Default)]
pub(super) struct CaseEvidence {
    pub(super) literal: bool,
    pub(super) uppercase: bool,
}
impl CaseEvidence {
    pub(super) fn character(&mut self, character: char) {
        self.literal = true;
        self.uppercase |= character.is_uppercase();
    }
    pub(super) fn literal(&mut self, value: &str) {
        for character in value.chars() {
            self.character(character);
        }
    }
    pub(super) fn merge(&mut self, other: Self) {
        self.literal |= other.literal;
        self.uppercase |= other.uppercase;
    }
}

pub(super) fn regex_case_evidence(value: &str) -> CaseEvidence {
    use regex_syntax::ast::{self, Ast, Visitor};
    struct LiteralVisitor(CaseEvidence);
    impl Visitor for LiteralVisitor {
        type Output = CaseEvidence;
        type Err = std::convert::Infallible;
        fn finish(self) -> Result<Self::Output, Self::Err> {
            Ok(self.0)
        }
        fn visit_pre(&mut self, node: &Ast) -> Result<(), Self::Err> {
            if let Ast::Literal(literal) = node {
                self.0.character(literal.c);
            }
            Ok(())
        }
        fn visit_class_set_item_pre(&mut self, item: &ast::ClassSetItem) -> Result<(), Self::Err> {
            match item {
                ast::ClassSetItem::Literal(literal) => self.0.character(literal.c),
                ast::ClassSetItem::Range(range) => {
                    self.0.character(range.start.c);
                    self.0.character(range.end.c);
                }
                _ => {}
            }
            Ok(())
        }
    }
    let Ok(tree) = ast::parse::Parser::new().parse(value) else {
        return CaseEvidence::default();
    };
    match ast::visit(&tree, LiteralVisitor(CaseEvidence::default())) {
        Ok(evidence) => evidence,
        Err(never) => match never {},
    }
}

/// One entire literal basename, independent of the query's text mode.
pub fn compile_exact_name(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    if value.is_empty() {
        return Err(QueryError::new(value.to_string(), "exact name is empty"));
    }
    regex::RegexBuilder::new(&format!(r"\A(?:{})\z", regex::escape(value)))
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

/// One literal line, preserving spaces and accepting LF/CR/CRLF terminators.
pub fn compile_content_line(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    if value.contains(['\n', '\r']) {
        return Err(QueryError::new(
            value.to_string(),
            "contentline expects a single line without line terminators",
        ));
    }
    let pattern = format!(r"(?mR:^(?:{})$)", regex::escape(value));
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

/// Literal terms with ripgrep-style Unicode half-word boundaries on both edges.
pub fn compile_content_word(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    if value.is_empty() {
        return Err(QueryError::new(
            value.to_string(),
            "word condition is empty",
        ));
    }
    let pattern = format!(
        r"\b{{start-half}}(?:{})\b{{end-half}}",
        regex::escape(value)
    );
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

pub fn extension_matches(name: &str, extension: &str) -> bool {
    name.strip_suffix(extension)
        .is_some_and(|prefix| prefix.ends_with('.'))
}

pub(super) fn valid_extension(value: &str) -> bool {
    !value.is_empty()
        && !value.split('.').any(str::is_empty)
        && !value
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | '\\' | '*' | '?'))
}

/// Compile Commander's whole-name `*`/`?` mask without changing Unicode character boundaries.
pub fn compile_name_glob(value: &str, case_sensitive: bool) -> Result<regex::Regex, QueryError> {
    let mut pattern = String::from("\\A");
    let mut literal_start = 0;
    for (index, character) in value.char_indices() {
        if matches!(character, '*' | '?') {
            pattern.push_str(&regex::escape(&value[literal_start..index]));
            pattern.push_str(if character == '*' { ".*" } else { "." });
            literal_start = index + 1;
        }
    }
    pattern.push_str(&regex::escape(&value[literal_start..]));
    pattern.push_str("\\z");
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .dot_matches_new_line(true)
        .build()
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}

/// Basenames match at any depth; relative paths use provider roots and absolute
/// Unix paths use the lexical filesystem root.
pub enum DirectoryMask {
    Name(regex::Regex),
    Relative(regex::Regex),
    Absolute(regex::Regex),
}

impl DirectoryMask {
    pub fn matches_name(&self, name: &str) -> bool {
        matches!(self, Self::Name(regex) if regex.is_match(name))
    }

    pub fn matches_directory(&self, path: &Path, root: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        if relative.as_os_str().is_empty() {
            return false;
        }
        match self {
            Self::Name(regex) => path
                .file_name()
                .is_some_and(|name| regex.is_match(&name.to_string_lossy())),
            Self::Absolute(regex) => {
                if !path.is_absolute() || !path.starts_with(Path::new("/")) {
                    return false;
                }
                let normalized = path
                    .components()
                    .filter_map(|component| match component {
                        std::path::Component::Normal(name) => Some(name.to_string_lossy()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/");
                regex.is_match(&normalized)
            }
            Self::Relative(regex) => {
                let normalized = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                regex.is_match(&normalized)
            }
        }
    }
}

pub fn compile_directory_mask(
    value: &str,
    case_sensitive: bool,
) -> Result<DirectoryMask, QueryError> {
    let invalid = || {
        QueryError::new(
            value.to_string(),
            "excludedir expects a name, relative or absolute directory mask without empty, . or .. components",
        )
    };
    if value.is_empty() {
        return Err(invalid());
    }
    if !value.contains('/') {
        return compile_name_glob(value, case_sensitive).map(DirectoryMask::Name);
    }
    let absolute = value.starts_with('/');
    let relative = if absolute {
        value.strip_prefix('/').unwrap()
    } else {
        value.strip_prefix("./").unwrap_or(value)
    };
    let relative = relative.strip_suffix('/').unwrap_or(relative);
    let mut parts = Vec::new();
    for part in relative.split('/') {
        if part.is_empty() || matches!(part, "." | "..") || (part.contains("**") && part != "**") {
            return Err(invalid());
        }
        if part == "**" && parts.last() == Some(&"**") {
            continue;
        }
        parts.push(part);
    }
    let mut pattern = String::from("\\A");
    for (index, part) in parts.iter().enumerate() {
        if *part == "**" {
            if index + 1 == parts.len() {
                pattern.push_str(if index == 0 {
                    "(?:[^/]+(?:/[^/]+)*)?"
                } else {
                    "(?:/[^/]+)*"
                });
            } else {
                if index > 0 {
                    pattern.push('/');
                }
                pattern.push_str("(?:[^/]+/)*");
            }
        } else {
            if index > 0 && parts[index - 1] != "**" {
                pattern.push('/');
            }
            let mut literal_start = 0;
            for (offset, character) in part.char_indices() {
                if matches!(character, '*' | '?') {
                    pattern.push_str(&regex::escape(&part[literal_start..offset]));
                    pattern.push_str(if character == '*' { "[^/]*" } else { "[^/]" });
                    literal_start = offset + 1;
                }
            }
            pattern.push_str(&regex::escape(&part[literal_start..]));
        }
    }
    pattern.push_str("\\z");
    regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map(|regex| {
            if absolute {
                DirectoryMask::Absolute(regex)
            } else {
                DirectoryMask::Relative(regex)
            }
        })
        .map_err(|error| QueryError::new(value.to_string(), error.to_string()))
}
