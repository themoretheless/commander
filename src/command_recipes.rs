//! Reviewed command starters. Choosing one only fills the existing command editor.
use crate::cmdtemplate::Template;

pub struct Recipe {
    pub name: &'static str,
    pub command: &'static str,
    pub requires: &'static str,
    pub extensions: &'static [&'static str],
}

pub const RECIPES: &[Recipe] = &[
    Recipe {
        name: "SHA-256 report",
        command: "shasum -a 256 {paths}",
        requires: "shasum",
        extensions: &[],
    },
    Recipe {
        name: "Identify file formats",
        command: "file {paths}",
        requires: "file",
        extensions: &[],
    },
    Recipe {
        name: "Count lines",
        command: "wc -l {paths}",
        requires: "wc",
        extensions: &[
            "txt", "md", "rs", "py", "c", "cpp", "h", "toml", "json", "csv",
        ],
    },
    Recipe {
        name: "Show disk usage",
        command: "du -sh {paths}",
        requires: "du",
        extensions: &[],
    },
    Recipe {
        name: "Inspect media metadata",
        command: "ffprobe -v error -show_format -show_streams {paths}",
        requires: "ffprobe (select one file)",
        extensions: &["mp4", "mov", "mkv", "webm", "mp3", "flac", "wav", "m4a"],
    },
];

pub fn parse_extensions(input: &str) -> Result<Vec<String>, &'static str> {
    if input.trim().is_empty() || input.trim() == "*" {
        return Ok(Vec::new());
    }
    let mut extensions = Vec::new();
    for raw in input
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .filter(|s| !s.is_empty())
    {
        let value = raw.trim_start_matches('.').to_ascii_lowercase();
        if value.is_empty()
            || value.len() > 64
            || value.starts_with('.')
            || value.ends_with('.')
            || !value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '-' | '_'))
        {
            return Err("Use extensions such as jpg, png or tar.gz; '*' alone means all files");
        }
        if !extensions.contains(&value) {
            extensions.push(value);
        }
        if extensions.len() > 32 {
            return Err("Use at most 32 extensions");
        }
    }
    if extensions.is_empty() {
        return Err("Enter at least one extension or *");
    }
    Ok(extensions)
}

pub fn template(line: &str, name: &str, extensions: &str) -> Result<Template, &'static str> {
    let raw = line.trim();
    if raw.is_empty() {
        return Err("Enter a command first");
    }
    let name = if name.trim().is_empty() {
        raw.split_whitespace().next().unwrap_or("Command")
    } else {
        name.trim()
    };
    Ok(Template {
        name: name.to_string(),
        raw: raw.to_string(),
        exts: parse_extensions(extensions)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn named_filtered_template_matches_compound_extensions_after_serialization() {
        let temp = crate::testutil::TempDir::new();
        let path = temp.file("PACKAGE.TAR.GZ", "bytes");
        let metadata = std::fs::metadata(&path).unwrap();
        let entry = crate::panel::FileEntry::from_meta(path, &metadata).unwrap();
        let saved = template("file {paths}", "Archive details", ".tar.gz, ZIP,zip").unwrap();
        assert_eq!(saved.exts, vec!["tar.gz", "zip"]);
        let decoded: Template =
            serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert!(decoded.matches(&[entry]));
        assert_eq!(decoded.name, "Archive details");
    }
    #[test]
    fn invalid_and_ambiguous_filters_are_rejected() {
        for invalid in ["*.jpg", "png;rm", "../jpg", "jpg|png", ",,,"] {
            assert!(parse_extensions(invalid).is_err(), "{invalid}");
        }
        assert!(parse_extensions("*").unwrap().is_empty());
        assert!(template("   ", "name", "").is_err());
    }
}
