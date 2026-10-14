//! Deterministic filesystem fixtures driven by a checked-in corpus profile.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WeightedCount {
    pub value: usize,
    pub weight: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WeightedSize {
    pub min_bytes: u64,
    pub max_bytes: u64,
    pub weight: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct WeightedFileType {
    pub extension: String,
    pub weight: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BenchmarkTreeProfile {
    pub schema: u32,
    pub name: String,
    pub source_note: String,
    pub seed: u64,
    pub entries: usize,
    pub max_total_bytes: u64,
    pub depths: Vec<WeightedCount>,
    pub fanouts: Vec<WeightedCount>,
    pub sizes: Vec<WeightedSize>,
    pub file_types: Vec<WeightedFileType>,
}

impl BenchmarkTreeProfile {
    pub fn validate(&self) -> Result<(), FixtureError> {
        if self.schema != 1 {
            return Err(FixtureError::Validation(
                "benchmark tree schema must be 1".to_string(),
            ));
        }
        if self.name.trim().is_empty() || self.source_note.trim().is_empty() {
            return Err(FixtureError::Validation(
                "benchmark tree profile needs a name and source note".to_string(),
            ));
        }
        if self.entries == 0 || self.max_total_bytes == 0 {
            return Err(FixtureError::Validation(
                "benchmark tree must contain entries and a byte budget".to_string(),
            ));
        }
        validate_counts("depth", &self.depths, 1, 32)?;
        validate_counts("fanout", &self.fanouts, 1, 4_096)?;
        if self.sizes.is_empty()
            || self
                .sizes
                .iter()
                .any(|size| size.weight == 0 || size.min_bytes > size.max_bytes)
        {
            return Err(FixtureError::Validation(
                "size buckets must be non-empty, weighted, and ordered".to_string(),
            ));
        }
        if self.file_types.is_empty()
            || self.file_types.iter().any(|file_type| {
                file_type.weight == 0
                    || file_type
                        .extension
                        .chars()
                        .any(|character| !character.is_ascii_alphanumeric())
            })
        {
            return Err(FixtureError::Validation(
                "file types must have weights and safe extensions".to_string(),
            ));
        }
        Ok(())
    }
}

fn validate_counts(
    label: &str,
    values: &[WeightedCount],
    minimum: usize,
    maximum: usize,
) -> Result<(), FixtureError> {
    if values.is_empty()
        || values
            .iter()
            .any(|value| value.weight == 0 || !(minimum..=maximum).contains(&value.value))
    {
        return Err(FixtureError::Validation(format!(
            "{label} distribution is empty or out of range"
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureEntry {
    pub relative_path: PathBuf,
    pub size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureManifest {
    pub profile: String,
    pub seed: u64,
    pub total_bytes: u64,
    pub entries: Vec<FixtureEntry>,
}

#[derive(Debug)]
pub enum FixtureError {
    Validation(String),
    Io(std::io::Error),
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(message) => formatter.write_str(message),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FixtureError {}

impl From<std::io::Error> for FixtureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn empirical_profile() -> Result<BenchmarkTreeProfile, FixtureError> {
    let profile = serde_json::from_str::<BenchmarkTreeProfile>(include_str!(
        "../ci/benchmark-tree-profile.json"
    ))
    .map_err(|error| FixtureError::Validation(error.to_string()))?;
    profile.validate()?;
    Ok(profile)
}

pub fn generate(profile: &BenchmarkTreeProfile) -> Result<FixtureManifest, FixtureError> {
    profile.validate()?;
    let mut random = DeterministicRandom(profile.seed);
    let mut entries = Vec::with_capacity(profile.entries);
    let mut sampled_total = 0_u128;
    for index in 0..profile.entries {
        let depth = choose_weighted(&mut random, &profile.depths, |value| value.weight).value;
        let fanout = choose_weighted(&mut random, &profile.fanouts, |value| value.weight).value;
        let size_bucket = choose_weighted(&mut random, &profile.sizes, |value| value.weight);
        let file_type = choose_weighted(&mut random, &profile.file_types, |value| value.weight);
        let mut relative_path = PathBuf::new();
        for level in 0..depth.saturating_sub(1) {
            let slot = random.next() % fanout as u64;
            relative_path.push(format!("d{level:02}-{slot:04}"));
        }
        let suffix = if file_type.extension.is_empty() {
            String::new()
        } else {
            format!(".{}", file_type.extension)
        };
        relative_path.push(format!("file-{index:06}{suffix}"));

        let size_bytes = random.range(size_bucket.min_bytes, size_bucket.max_bytes);
        sampled_total = sampled_total.saturating_add(u128::from(size_bytes));
        entries.push(FixtureEntry {
            relative_path,
            size_bytes,
        });
    }
    // Scale every entry down uniformly when the sample overshoots the budget,
    // so the size mix keeps its shape instead of zeroing the tail entries.
    if sampled_total > u128::from(profile.max_total_bytes) {
        for entry in &mut entries {
            let scaled =
                u128::from(entry.size_bytes) * u128::from(profile.max_total_bytes) / sampled_total;
            entry.size_bytes = u64::try_from(scaled).unwrap_or(u64::MAX);
        }
    }
    let total_bytes = entries.iter().map(|entry| entry.size_bytes).sum();
    Ok(FixtureManifest {
        profile: profile.name.clone(),
        seed: profile.seed,
        total_bytes,
        entries,
    })
}

/// Writes the manifest's tree under `root`, which must be missing or empty so
/// leftovers from an earlier manifest cannot inflate the benchmark tree.
pub fn materialize(root: &Path, manifest: &FixtureManifest) -> Result<(), FixtureError> {
    std::fs::create_dir_all(root)?;
    if std::fs::read_dir(root)?.next().is_some() {
        return Err(FixtureError::Validation(format!(
            "benchmark fixture root {} is not empty",
            root.display()
        )));
    }
    for entry in &manifest.entries {
        let path = root.join(&entry.relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::File::create(path)?.set_len(entry.size_bytes)?;
    }
    Ok(())
}

fn choose_weighted<'a, T>(
    random: &mut DeterministicRandom,
    values: &'a [T],
    weight: impl Fn(&T) -> u32,
) -> &'a T {
    let total = values
        .iter()
        .map(|value| u64::from(weight(value)))
        .sum::<u64>();
    let mut target = random.next() % total;
    for value in values {
        let current = u64::from(weight(value));
        if target < current {
            return value;
        }
        target -= current;
    }
    &values[values.len() - 1]
}

struct DeterministicRandom(u64);

impl DeterministicRandom {
    const fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    const fn range(&mut self, minimum: u64, maximum: u64) -> u64 {
        if minimum == maximum {
            return minimum;
        }
        let width = maximum - minimum;
        if width == u64::MAX {
            self.next()
        } else {
            minimum + self.next() % (width + 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn small_profile() -> BenchmarkTreeProfile {
        BenchmarkTreeProfile {
            schema: 1,
            name: "test-mix".to_string(),
            source_note: "deterministic test distribution".to_string(),
            seed: 17,
            entries: 24,
            max_total_bytes: 2_048,
            depths: vec![
                WeightedCount {
                    value: 1,
                    weight: 1,
                },
                WeightedCount {
                    value: 3,
                    weight: 1,
                },
            ],
            fanouts: vec![WeightedCount {
                value: 4,
                weight: 1,
            }],
            sizes: vec![WeightedSize {
                min_bytes: 1,
                max_bytes: 256,
                weight: 1,
            }],
            file_types: vec![
                WeightedFileType {
                    extension: "txt".to_string(),
                    weight: 2,
                },
                WeightedFileType {
                    extension: String::new(),
                    weight: 1,
                },
            ],
        }
    }

    #[test]
    fn checked_in_empirical_profile_is_valid() {
        let profile = empirical_profile().unwrap();
        assert_eq!(profile.schema, 1);
        assert!(profile.entries >= 1_000);
        assert!(profile.depths.len() > 3);
        assert!(profile.fanouts.len() > 3);
        assert!(profile.sizes.len() > 3);
        assert!(profile.file_types.len() > 6);
    }

    #[test]
    fn generation_is_deterministic_and_respects_byte_budget() {
        let profile = small_profile();
        let first = generate(&profile).unwrap();
        let second = generate(&profile).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.entries.len(), profile.entries);
        assert!(first.total_bytes <= profile.max_total_bytes);
        assert!(
            first
                .entries
                .iter()
                .any(|entry| entry.relative_path.components().count() > 1)
        );
    }

    #[test]
    fn over_budget_sizes_scale_down_instead_of_zeroing_the_tail() {
        let mut profile = small_profile();
        profile.sizes = vec![WeightedSize {
            min_bytes: 128,
            max_bytes: 256,
            weight: 1,
        }];
        let manifest = generate(&profile).unwrap();
        assert!(manifest.total_bytes <= profile.max_total_bytes);
        assert_eq!(
            manifest.total_bytes,
            manifest
                .entries
                .iter()
                .map(|entry| entry.size_bytes)
                .sum::<u64>()
        );
        assert!(manifest.entries.iter().all(|entry| entry.size_bytes > 0));
    }

    #[test]
    fn materialize_rejects_a_non_empty_root() {
        let temp = TempDir::new();
        std::fs::write(temp.path().join("stale.bin"), b"old").unwrap();
        let manifest = generate(&small_profile()).unwrap();
        assert!(matches!(
            materialize(temp.path(), &manifest),
            Err(FixtureError::Validation(_))
        ));
    }

    #[test]
    fn manifest_materializes_the_profiled_tree() {
        let temp = TempDir::new();
        let manifest = generate(&small_profile()).unwrap();
        materialize(temp.path(), &manifest).unwrap();
        for entry in &manifest.entries {
            let metadata = std::fs::metadata(temp.path().join(&entry.relative_path)).unwrap();
            assert_eq!(metadata.len(), entry.size_bytes);
        }
    }
}
