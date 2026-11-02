//! Explicit measurement policy and loss-aware aggregation for recursive usage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    #[default]
    Logical,
    FileAllocation,
    FileCount,
}
impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Self::Logical => "Logical size",
            Self::FileAllocation => "File allocation",
            Self::FileCount => "File count",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Logical => "logical",
            Self::FileAllocation => "file_allocation",
            Self::FileCount => "file_count",
        }
    }
    pub fn unit(self) -> &'static str {
        if self == Self::FileCount {
            "entries"
        } else {
            "bytes"
        }
    }
    pub fn text(self, value: u64, status: Measurement) -> String {
        if self == Self::FileCount {
            format!(
                "{}{value} entries",
                if status.complete() { "" } else { "≥" }
            )
        } else {
            status.text(value)
        }
    }
    /// Directories are aggregate containers, never measured here. Allocation
    /// counts regular-file data blocks; links/special entries contribute zero.
    pub fn observe(self, metadata: &std::fs::Metadata) -> Option<u64> {
        match self {
            Self::Logical => Some(metadata.len()),
            Self::FileCount => Some(1),
            Self::FileAllocation if metadata.is_file() => crate::panel::allocated_size(metadata),
            Self::FileAllocation => Some(0),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Measurement {
    pub unknown: usize,
    pub overflowed: bool,
}
impl Measurement {
    pub fn unknown() -> Self {
        Self {
            unknown: 1,
            overflowed: false,
        }
    }
    pub fn complete(self) -> bool {
        self.unknown == 0 && !self.overflowed
    }
    pub fn add(&mut self, bytes: &mut u64, more: u64, status: Self) {
        *bytes = bytes.checked_add(more).unwrap_or_else(|| {
            self.overflowed = true;
            u64::MAX
        });
        self.unknown = self.unknown.checked_add(status.unknown).unwrap_or_else(|| {
            self.overflowed = true;
            usize::MAX
        });
        self.overflowed |= status.overflowed;
    }
    pub fn value(self, bytes: u64) -> Option<u64> {
        self.complete().then_some(bytes)
    }
    pub fn text(self, bytes: u64) -> String {
        format!(
            "{}{}",
            if self.complete() { "" } else { "≥" },
            crate::panel::format_size(bytes)
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_and_overflowed_measurements_never_become_complete_zero() {
        let mut state = Measurement::default();
        let mut bytes = 0;
        state.add(&mut bytes, 0, Measurement::default());
        assert_eq!(state.value(bytes), Some(0));
        state.add(&mut bytes, 8, Measurement::unknown());
        assert_eq!(bytes, 8);
        assert_eq!(state.unknown, 1);
        assert_eq!(state.value(bytes), None);
        state.add(&mut bytes, u64::MAX, Measurement::default());
        assert_eq!(bytes, u64::MAX);
        assert!(state.overflowed);
        assert_eq!(state.value(bytes), None);
        let mut count = Measurement {
            unknown: usize::MAX,
            overflowed: false,
        };
        count.add(&mut bytes, 0, Measurement::unknown());
        assert!(count.overflowed);
        assert_eq!(count.unknown, usize::MAX);
    }
}
