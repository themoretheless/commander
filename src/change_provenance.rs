//! Path-free change provenance for refreshed rows (research **J007**).
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeProvenance { #[default] Unknown, Commander, ExternalWatcher, Reconciliation, Recovery }
impl ChangeProvenance {
    pub const fn label(self) -> &'static str {
        match self { Self::Unknown => "Unknown", Self::Commander => "Commander", Self::ExternalWatcher => "External watcher", Self::Reconciliation => "Reconciliation", Self::Recovery => "Recovery" }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowProvenance { pub generation: u64, pub source: ChangeProvenance }
impl RowProvenance {
    pub fn mark(mut self, source: ChangeProvenance) -> Self { self.generation = self.generation.saturating_add(1); self.source = source; self }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mark_advances_generation_and_source() {
        let row = RowProvenance::default().mark(ChangeProvenance::ExternalWatcher);
        assert_eq!(row.generation, 1);
        assert_eq!(row.source, ChangeProvenance::ExternalWatcher);
    }
}
