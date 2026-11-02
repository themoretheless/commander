//! Cached, read-only projection of completed checksum-list rows.
use crate::checksum_list::{Report, Status};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Problems,
    Mismatches,
    Errors,
    Matches,
}
impl Filter {
    pub const ALL: [Self; 5] = [
        Self::All,
        Self::Problems,
        Self::Mismatches,
        Self::Errors,
        Self::Matches,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Problems => "Problems",
            Self::Mismatches => "Mismatches",
            Self::Errors => "Errors",
            Self::Matches => "Matches",
        }
    }
    fn accepts(self, status: &Status) -> bool {
        match self {
            Self::All => true,
            Self::Problems => !matches!(status, Status::Match),
            Self::Mismatches => matches!(status, Status::Mismatch),
            Self::Errors => matches!(status, Status::Error(_)),
            Self::Matches => matches!(status, Status::Match),
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub filter: Filter,
    pub query: String,
}
#[derive(Default)]
pub struct View {
    pub options: Options,
    pub indices: Vec<usize>,
    ready: bool,
    applied: Options,
}
impl View {
    pub fn invalidate(&mut self) {
        self.ready = false;
    }
    pub fn is_current(&self) -> bool {
        self.ready && self.options == self.applied
    }
    pub fn refresh(&mut self, report: &Report) -> bool {
        if self.is_current() {
            return false;
        }
        self.applied = self.options.clone();
        self.ready = true;
        self.indices.clear();
        if self.options.query.chars().take(257).count() > 256 {
            return true;
        }
        let query = self.options.query.trim().to_lowercase();
        self.indices = report
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                (self.options.filter.accepts(&row.status)
                    && (query.is_empty() || row.name.to_lowercase().contains(&query)))
                .then_some(index)
            })
            .collect();
        true
    }
}

const COPY_LIMIT: usize = 1024 * 1024;
#[derive(Default)]
struct Output(String);
impl std::fmt::Write for Output {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        if text.len() > COPY_LIMIT.saturating_sub(self.0.len()) {
            return Err(std::fmt::Error);
        }
        self.0.push_str(text);
        Ok(())
    }
}
/// Copies only shown rows, with full-report and partial-verification context.
pub fn visible_text(report: &Report, view: &View) -> Result<String, String> {
    if !view.is_current() {
        return Err("Wait for the displayed report filter to update".into());
    }
    if view.options.query.chars().take(257).count() > 256 {
        return Err("Search is limited to 256 characters".into());
    }
    let mut projected = View {
        options: view.options.clone(),
        ..Default::default()
    };
    projected.refresh(report);
    if projected.indices != view.indices {
        return Err("Displayed checksum rows are stale; update the view before copying".into());
    }
    if view.indices.is_empty() {
        return Err("No visible checksum rows to copy".into());
    }
    let mut output = Output::default();
    let write = |output: &mut Output| -> std::fmt::Result {
        writeln!(
            output,
            "{} checksum list",
            report
                .algorithm
                .map_or("Undetermined", crate::checksum::Algorithm::label)
        )?;
        writeln!(output, "{}", report.summary())?;
        writeln!(
            output,
            "Visible records: {} of {} completed; status={}; query=\"{}\"",
            view.indices.len(),
            report.rows.len(),
            view.options.filter.label(),
            view.options.query.escape_debug()
        )?;
        for &index in &view.indices {
            crate::checksum_list::write_row(output, &report.rows[index])?;
        }
        Ok(())
    };
    write(&mut output)
        .map_err(|_| "Visible report exceeds 1 MiB; narrow the status/name filter".to_string())?;
    Ok(output.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{checksum::Algorithm, checksum_list::Row};
    fn report() -> Report {
        Report {
            rows: vec![
                Row {
                    name: "папка/Пример".into(),
                    expected: "a".into(),
                    actual: Some("a".into()),
                    status: Status::Match,
                },
                Row {
                    name: "nested/mismatch".into(),
                    expected: "a".into(),
                    actual: Some("b".into()),
                    status: Status::Mismatch,
                },
                Row {
                    name: "nested/missing\nname".into(),
                    expected: "a".into(),
                    actual: None,
                    status: Status::Error("missing\nfile".into()),
                },
            ],
            total: 5,
            cancelled: true,
            error: None,
            algorithm: Some(Algorithm::Md5),
        }
    }
    #[test]
    fn statuses_and_unicode_names_compose_with_stable_order_and_cached_empty_views() {
        let report = report();
        let mut view = View::default();
        for (filter, expected) in [
            (Filter::All, vec![0, 1, 2]),
            (Filter::Problems, vec![1, 2]),
            (Filter::Mismatches, vec![1]),
            (Filter::Errors, vec![2]),
            (Filter::Matches, vec![0]),
        ] {
            view.options.filter = filter;
            view.refresh(&report);
            assert_eq!(view.indices, expected);
        }
        view.options.query = "ПРИМЕР".into();
        view.refresh(&report);
        assert_eq!(view.indices, [0]);
        view.options.filter = Filter::Problems;
        view.refresh(&report);
        assert!(view.indices.is_empty());
        assert!(!view.refresh(&report));
        view.options.query = "missing".into();
        view.refresh(&report);
        assert_eq!(view.indices, [2]);
        view.options.query = "x".repeat(257);
        view.refresh(&report);
        assert!(view.indices.is_empty());
        assert_eq!(report.rows.len(), 3);
        assert_eq!(report.total, 5);
    }
    #[test]
    fn new_same_length_reports_require_invalidation_and_keep_preferences() {
        let mut report = report();
        let mut view = View::default();
        view.options.query = "missing".into();
        view.refresh(&report);
        assert_eq!(view.indices, [2]);
        report.rows[2].name = "renamed".into();
        view.invalidate();
        view.refresh(&report);
        assert!(view.indices.is_empty());
        assert_eq!(view.options.query, "missing");
        assert!(!view.refresh(&report));
    }
    #[test]
    fn visible_copy_keeps_full_summary_partial_status_and_escaped_names_but_refuses_stale_views() {
        let mut report = report();
        let mut view = View::default();
        view.options.filter = Filter::Errors;
        view.refresh(&report);
        let text = visible_text(&report, &view).unwrap();
        assert!(text.contains("Stopped: 3 of 5 records: 1 match, 1 mismatch, 1 failed"));
        assert!(text.contains("Visible records: 1 of 3 completed; status=Errors; query=\"\""));
        assert!(text.contains("ERROR nested/missing\\nname expected=a actual=- (missing\\nfile)"));
        assert!(!text.contains("MISMATCH ") && !text.contains("MATCH "));
        view.indices.push(999);
        assert!(visible_text(&report, &view).is_err());
        view.invalidate();
        view.refresh(&report);
        report.rows[2].status = Status::Match;
        assert!(visible_text(&report, &view).is_err());
        view.invalidate();
        view.refresh(&report);
        assert!(visible_text(&report, &view).is_err());
        view.options.filter = Filter::All;
        assert!(visible_text(&report, &view).is_err());
    }
    #[test]
    fn visible_copy_limits_actual_formatted_output() {
        let mut report = report();
        report.cancelled = false;
        let mut row = report.rows[0].clone();
        row.name = "a".repeat(50);
        row.expected = "a".repeat(32);
        row.actual = Some("a".repeat(32));
        report.rows = vec![row; 10_000];
        report.total = 10_000;
        let mut view = View::default();
        view.refresh(&report);
        assert!(visible_text(&report, &view).unwrap_err().contains("1 MiB"));
    }
}
