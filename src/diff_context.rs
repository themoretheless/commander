//! Compact context projection; all row identities remain in the original diff.
use crate::diff_navigation::Hunk;
use std::{collections::BTreeSet, ops::Range};
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Line(usize),
    Gap(Range<usize>),
}
struct Chunk {
    source: Range<usize>,
    display: usize,
    hidden: bool,
}
pub struct Context {
    pub compact: bool,
    pub context: usize,
    source_len: usize,
    chunks: Vec<Chunk>,
    expanded: BTreeSet<(usize, usize)>,
    len: usize,
    pub hidden: usize,
}
impl Default for Context {
    fn default() -> Self {
        Self {
            compact: false,
            context: 3,
            source_len: 0,
            chunks: vec![],
            expanded: BTreeSet::new(),
            len: 0,
            hidden: 0,
        }
    }
}
impl Context {
    pub fn row_count(&self) -> usize {
        self.len
    }
    pub fn row(&self, index: usize) -> Option<Row> {
        if index >= self.len {
            return None;
        }
        let position = self
            .chunks
            .partition_point(|chunk| chunk.display <= index)
            .checked_sub(1)?;
        let chunk = &self.chunks[position];
        Some(if chunk.hidden {
            Row::Gap(chunk.source.clone())
        } else {
            Row::Line(chunk.source.start + index - chunk.display)
        })
    }
    pub fn display_index(&self, source: usize) -> Option<usize> {
        if source >= self.source_len {
            return None;
        }
        let chunk = self.chunks.get(
            self.chunks
                .partition_point(|chunk| chunk.source.end <= source),
        )?;
        Some(
            chunk.display
                + if chunk.hidden {
                    0
                } else {
                    source - chunk.source.start
                },
        )
    }
    pub fn rebuild(&mut self, len: usize, hunks: &[Hunk]) {
        self.source_len = len;
        self.chunks.clear();
        self.len = 0;
        self.hidden = 0;
        let mut kept: Vec<Range<usize>> = Vec::new();
        if !self.compact || hunks.is_empty() {
            kept.push(0..len);
        } else {
            for hunk in hunks {
                let range = hunk.rows.start.saturating_sub(self.context)
                    ..hunk.rows.end.saturating_add(self.context).min(len);
                if let Some(last) = kept.last_mut()
                    && last.end >= range.start
                {
                    last.end = last.end.max(range.end);
                } else {
                    kept.push(range);
                }
            }
        }
        let mut cursor = 0;
        for range in kept {
            if cursor < range.start {
                self.push(cursor..range.start, true);
            }
            cursor = range.end;
            self.push(range, false);
        }
        if cursor < len {
            self.push(cursor..len, true);
        }
    }
    fn push(&mut self, range: Range<usize>, gap: bool) {
        if range.is_empty() {
            return;
        }
        let hidden = gap && !self.expanded.contains(&(range.start, range.end));
        self.chunks.push(Chunk {
            source: range.clone(),
            display: self.len,
            hidden,
        });
        self.len += if hidden { 1 } else { range.len() };
        if hidden {
            self.hidden += range.len();
        }
    }
    pub fn reveal(&mut self, source: usize, hunks: &[Hunk]) -> bool {
        let Some(index) = self.display_index(source) else {
            return false;
        };
        if let Some(Row::Gap(range)) = self.row(index) {
            self.expand(range, hunks)
        } else {
            false
        }
    }
    pub fn expand(&mut self, range: Range<usize>, hunks: &[Hunk]) -> bool {
        if !self
            .chunks
            .iter()
            .any(|chunk| chunk.hidden && chunk.source == range)
        {
            return false;
        }
        self.expanded.insert((range.start, range.end));
        self.rebuild(self.source_len, hunks);
        true
    }
    pub fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        len: usize,
        hunks: &[Hunk],
    ) -> (bool, Vec<egui::Id>) {
        let mut changed = false;
        let mut ids = Vec::new();
        ui.horizontal_wrapped(|ui| {
            let compact = ui.checkbox(&mut self.compact, "Compact diff");
            changed |= compact.changed();
            ids.push(compact.id);
            ui.label("Context lines");
            let context = ui.add_enabled(
                self.compact,
                egui::DragValue::new(&mut self.context).range(0..=50),
            );
            changed |= context.changed();
            ids.push(context.id);
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new("diff-compact"), compact.rect);
                data.insert_temp(egui::Id::new("diff-context-lines"), context.rect);
            });
        });
        if changed {
            self.expanded.clear();
        }
        if changed || self.source_len != len {
            self.rebuild(len, hunks);
        }
        ui.small(format!(
            "{} / {} display rows · {} unchanged lines hidden",
            self.len, len, self.hidden
        ));
        (changed, ids)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn hunk(rows: Range<usize>) -> Hunk {
        Hunk {
            rows,
            left_start: 1,
            left_len: 1,
            right_start: 1,
            right_len: 1,
        }
    }
    #[test]
    fn context_merges_ranges_preserves_changed_ids_and_expands_only_exact_hidden_blocks() {
        let hunks = vec![hunk(10..12), hunk(14..16), hunk(90..92)];
        let mut view = Context {
            compact: true,
            ..Default::default()
        };
        view.rebuild(100, &hunks);
        let rows: Vec<_> = (0..view.row_count())
            .map(|i| view.row(i).unwrap())
            .collect();
        assert_eq!(rows.first(), Some(&Row::Gap(0..7)));
        assert_eq!(rows.last(), Some(&Row::Gap(95..100)));
        assert!(rows.contains(&Row::Gap(19..87)));
        for index in [10, 11, 14, 15, 90, 91] {
            let display = view.display_index(index).unwrap();
            assert_eq!(view.row(display), Some(Row::Line(index)));
        }
        let hidden = view.hidden;
        assert!(!view.expand(20..87, &hunks));
        assert!(view.expand(19..87, &hunks));
        assert_eq!(view.hidden, hidden - 68);
        assert!(!view.expand(19..87, &hunks));
        view.compact = false;
        view.rebuild(100, &hunks);
        assert_eq!(view.row_count(), 100);
        assert_eq!(view.hidden, 0);
        for i in 0..100 {
            assert_eq!(view.row(i), Some(Row::Line(i)));
        }
    }
    #[test]
    fn zero_context_and_edges_keep_every_change_and_no_fake_differences_for_identical_files() {
        let mut view = Context {
            compact: true,
            context: 0,
            ..Default::default()
        };
        let hunks = vec![hunk(0..2), hunk(8..10)];
        view.rebuild(10, &hunks);
        assert_eq!(
            (0..view.row_count())
                .map(|i| view.row(i).unwrap())
                .collect::<Vec<_>>(),
            [
                Row::Line(0),
                Row::Line(1),
                Row::Gap(2..8),
                Row::Line(8),
                Row::Line(9)
            ]
        );
        view.rebuild(10, &[]);
        assert_eq!(view.row_count(), 10);
        assert_eq!(view.hidden, 0);
        view.rebuild(0, &[]);
        assert_eq!(view.row_count(), 0);
        assert!(view.row(0).is_none());
        assert!(view.display_index(0).is_none());
    }
}
