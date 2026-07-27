//! Bounded runtime telemetry and deterministic CI performance gates.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_METRIC_SAMPLES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricName {
    StartupTotal,
    FirstListing,
    FilterResponse,
    OperationDialog,
    FrameTime,
}

impl MetricName {
    pub const CI: [Self; 4] = [
        Self::StartupTotal,
        Self::FirstListing,
        Self::FilterResponse,
        Self::OperationDialog,
    ];
    pub const ALL: [Self; 5] = [
        Self::StartupTotal,
        Self::FirstListing,
        Self::FilterResponse,
        Self::OperationDialog,
        Self::FrameTime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::StartupTotal => "Startup",
            Self::FirstListing => "First listing",
            Self::FilterResponse => "Filter response",
            Self::OperationDialog => "Operation dialog",
            Self::FrameTime => "Frame time",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LatencyPercentiles {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub samples: usize,
}

pub fn latency_percentiles(samples: &[f64]) -> LatencyPercentiles {
    let mut sorted = samples
        .iter()
        .copied()
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect::<Vec<_>>();
    sorted.sort_by(f64::total_cmp);
    LatencyPercentiles {
        p50_ms: percentile_f64(&sorted, 0.50),
        p95_ms: percentile_f64(&sorted, 0.95),
        p99_ms: percentile_f64(&sorted, 0.99),
        samples: sorted.len(),
    }
}

fn percentile_f64(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (quantile * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn percentiles_u64(samples: &VecDeque<u64>) -> (u64, u64, u64) {
    let mut sorted = samples.iter().copied().collect::<Vec<_>>();
    sorted.sort_unstable();
    if sorted.is_empty() {
        return (0, 0, 0);
    }
    (
        percentile_sorted_u64(&sorted, 0.50),
        percentile_sorted_u64(&sorted, 0.95),
        percentile_sorted_u64(&sorted, 0.99),
    )
}

fn percentile_sorted_u64(sorted: &[u64], quantile: f64) -> u64 {
    let rank = (quantile.clamp(0.0, 1.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn telemetry() -> &'static Mutex<HashMap<MetricName, VecDeque<f64>>> {
    static TELEMETRY: OnceLock<Mutex<HashMap<MetricName, VecDeque<f64>>>> = OnceLock::new();
    TELEMETRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn record(metric: MetricName, elapsed: Duration) {
    record_ms(metric, elapsed.as_secs_f64() * 1_000.0);
}

pub fn record_ms(metric: MetricName, elapsed_ms: f64) {
    if !elapsed_ms.is_finite() || elapsed_ms < 0.0 {
        return;
    }
    let mut telemetry = crate::lock_util::recover(telemetry());
    let samples = telemetry.entry(metric).or_default();
    samples.push_back(elapsed_ms);
    while samples.len() > MAX_METRIC_SAMPLES {
        samples.pop_front();
    }
}

pub fn snapshot(metric: MetricName) -> LatencyPercentiles {
    let telemetry = crate::lock_util::recover(telemetry());
    let Some(samples) = telemetry.get(&metric) else {
        return LatencyPercentiles::default();
    };
    let samples = samples.iter().copied().collect::<Vec<_>>();
    latency_percentiles(&samples)
}

pub fn snapshots() -> Vec<(MetricName, LatencyPercentiles)> {
    let telemetry = crate::lock_util::recover(telemetry());
    MetricName::ALL
        .into_iter()
        .map(|metric| {
            let latency =
                telemetry
                    .get(&metric)
                    .map_or_else(LatencyPercentiles::default, |samples| {
                        let samples = samples.iter().copied().collect::<Vec<_>>();
                        latency_percentiles(&samples)
                    });
            (metric, latency)
        })
        .collect()
}

pub struct LatencyGuard {
    metric: MetricName,
    started: Instant,
}

impl LatencyGuard {
    pub fn new(metric: MetricName) -> Self {
        Self {
            metric,
            started: Instant::now(),
        }
    }
}

impl Drop for LatencyGuard {
    fn drop(&mut self) {
        record(self.metric, self.started.elapsed());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupPhase {
    SessionRestore,
    Appearance,
    WorkspaceRestore,
    RecoveryScan,
    StoreLoad,
    AppAssembly,
    FirstListing,
}

impl StartupPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::SessionRestore => "Session restore",
            Self::Appearance => "Appearance",
            Self::WorkspaceRestore => "Workspace restore",
            Self::RecoveryScan => "Recovery scan",
            Self::StoreLoad => "Store load",
            Self::AppAssembly => "App assembly",
            Self::FirstListing => "First listing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartupSpan {
    pub phase: StartupPhase,
    pub duration_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StartupSnapshot {
    pub total_ms: f64,
    pub phases: Vec<StartupSpan>,
}

impl StartupSnapshot {
    pub fn phase(&self, phase: StartupPhase) -> Option<f64> {
        self.phases
            .iter()
            .find(|span| span.phase == phase)
            .map(|span| span.duration_ms)
    }
}

pub struct StartupTrace {
    started: Instant,
    checkpoint: Instant,
    phases: Vec<StartupSpan>,
}

impl StartupTrace {
    pub fn start() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            checkpoint: now,
            phases: Vec::new(),
        }
    }

    pub fn checkpoint(&mut self, phase: StartupPhase) {
        let now = Instant::now();
        self.phases.push(StartupSpan {
            phase,
            duration_ms: now.duration_since(self.checkpoint).as_secs_f64() * 1_000.0,
        });
        self.checkpoint = now;
    }

    pub fn finish(self) -> StartupSnapshot {
        let snapshot = StartupSnapshot {
            total_ms: self.started.elapsed().as_secs_f64() * 1_000.0,
            phases: self.phases,
        };
        record_ms(MetricName::StartupTotal, snapshot.total_ms);
        *crate::lock_util::recover(latest_startup_slot()) = Some(snapshot.clone());
        snapshot
    }
}

fn latest_startup_slot() -> &'static Mutex<Option<StartupSnapshot>> {
    static STARTUP: OnceLock<Mutex<Option<StartupSnapshot>>> = OnceLock::new();
    STARTUP.get_or_init(|| Mutex::new(None))
}

pub fn latest_startup() -> Option<StartupSnapshot> {
    crate::lock_util::recover(latest_startup_slot()).clone()
}

#[derive(Clone, Debug, Deserialize)]
pub struct PerformanceBudgets {
    pub schema: u32,
    pub metrics: Vec<MetricBudget>,
    #[serde(default)]
    pub regression_explanations: Vec<RegressionExplanation>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MetricBudget {
    pub metric: MetricName,
    pub hard_p95_ms: f64,
    pub baseline_p95_ms: f64,
    pub allowed_regression_percent: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegressionExplanation {
    pub metric: MetricName,
    pub owner: String,
    pub reason: String,
    pub tracking_issue: String,
    pub expires_on: String,
}

impl RegressionExplanation {
    fn actionable(&self) -> bool {
        let complete = [
            self.owner.as_str(),
            self.reason.as_str(),
            self.tracking_issue.as_str(),
            self.expires_on.as_str(),
        ]
        .into_iter()
        .all(|value| !value.trim().is_empty());
        complete
            && chrono::NaiveDate::parse_from_str(&self.expires_on, "%Y-%m-%d")
                .is_ok_and(|expiry| expiry >= chrono::Local::now().date_naive())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub schema: u32,
    pub fixture_profile: String,
    pub metrics: Vec<BenchmarkMetric>,
    pub cancellation_latency: LatencyPercentiles,
    pub stale_results: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BenchmarkMetric {
    pub metric: MetricName,
    pub latency: LatencyPercentiles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateViolationKind {
    InvalidContract,
    MissingMetric,
    InvalidPercentiles,
    HardBudget,
    UnexplainedRegression,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GateViolation {
    pub metric: Option<MetricName>,
    pub kind: GateViolationKind,
    pub detail: String,
}

impl std::fmt::Display for GateViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.metric {
            Some(metric) => write!(formatter, "{}: {}", metric.label(), self.detail),
            None => formatter.write_str(&self.detail),
        }
    }
}

pub fn evaluate_performance_gate(
    budgets: &PerformanceBudgets,
    report: &BenchmarkReport,
) -> Vec<GateViolation> {
    let mut violations = Vec::new();
    if budgets.schema != 1 || report.schema != 1 {
        violations.push(GateViolation {
            metric: None,
            kind: GateViolationKind::InvalidContract,
            detail: "performance budget and report schemas must both be 1".to_string(),
        });
    }
    if report.fixture_profile.trim().is_empty() {
        violations.push(GateViolation {
            metric: None,
            kind: GateViolationKind::InvalidContract,
            detail: "benchmark report must name its fixture profile".to_string(),
        });
    }

    let mut report_metrics = HashSet::new();
    for measurement in &report.metrics {
        if !report_metrics.insert(measurement.metric) {
            violations.push(GateViolation {
                metric: Some(measurement.metric),
                kind: GateViolationKind::InvalidContract,
                detail: "duplicate benchmark measurement".to_string(),
            });
        }
    }
    let cancellation = report.cancellation_latency;
    if cancellation.samples > 0
        && (!cancellation.p50_ms.is_finite()
            || !cancellation.p95_ms.is_finite()
            || !cancellation.p99_ms.is_finite()
            || cancellation.p50_ms < 0.0
            || cancellation.p50_ms > cancellation.p95_ms
            || cancellation.p95_ms > cancellation.p99_ms)
    {
        violations.push(GateViolation {
            metric: None,
            kind: GateViolationKind::InvalidPercentiles,
            detail: "cancellation percentiles must be finite and ordered".to_string(),
        });
    }

    let mut seen = HashSet::new();
    for budget in &budgets.metrics {
        if !seen.insert(budget.metric) {
            violations.push(GateViolation {
                metric: Some(budget.metric),
                kind: GateViolationKind::InvalidContract,
                detail: "duplicate metric budget".to_string(),
            });
            continue;
        }
        let Some(measurement) = report
            .metrics
            .iter()
            .find(|measurement| measurement.metric == budget.metric)
        else {
            violations.push(GateViolation {
                metric: Some(budget.metric),
                kind: GateViolationKind::MissingMetric,
                detail: "benchmark measurement is missing".to_string(),
            });
            continue;
        };
        let latency = measurement.latency;
        if latency.samples == 0
            || !latency.p50_ms.is_finite()
            || !latency.p95_ms.is_finite()
            || !latency.p99_ms.is_finite()
            || latency.p50_ms < 0.0
            || latency.p50_ms > latency.p95_ms
            || latency.p95_ms > latency.p99_ms
        {
            violations.push(GateViolation {
                metric: Some(budget.metric),
                kind: GateViolationKind::InvalidPercentiles,
                detail: "latency percentiles must be finite, ordered, and sampled".to_string(),
            });
            continue;
        }
        if latency.p95_ms > budget.hard_p95_ms {
            violations.push(GateViolation {
                metric: Some(budget.metric),
                kind: GateViolationKind::HardBudget,
                detail: format!(
                    "p95 {:.2} ms exceeds hard budget {:.2} ms",
                    latency.p95_ms, budget.hard_p95_ms
                ),
            });
            continue;
        }
        let regression_limit =
            budget.baseline_p95_ms * (1.0 + budget.allowed_regression_percent / 100.0);
        let explained = budgets
            .regression_explanations
            .iter()
            .any(|explanation| explanation.metric == budget.metric && explanation.actionable());
        if latency.p95_ms > regression_limit && !explained {
            violations.push(GateViolation {
                metric: Some(budget.metric),
                kind: GateViolationKind::UnexplainedRegression,
                detail: format!(
                    "p95 {:.2} ms exceeds regression limit {:.2} ms without an owner and issue",
                    latency.p95_ms, regression_limit
                ),
            });
        }
    }
    violations
}

pub fn ci_budgets() -> Result<PerformanceBudgets, serde_json::Error> {
    serde_json::from_str(include_str!("../ci/performance-budgets.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn compliant_report(budgets: &PerformanceBudgets) -> BenchmarkReport {
        BenchmarkReport {
            schema: 1,
            fixture_profile: "unit-contract".to_string(),
            metrics: budgets
                .metrics
                .iter()
                .map(|budget| BenchmarkMetric {
                    metric: budget.metric,
                    latency: LatencyPercentiles {
                        p50_ms: budget.baseline_p95_ms * 0.8,
                        p95_ms: budget.baseline_p95_ms,
                        p99_ms: budget.baseline_p95_ms * 1.05,
                        samples: 30,
                    },
                })
                .collect(),
            cancellation_latency: LatencyPercentiles {
                samples: 1,
                ..LatencyPercentiles::default()
            },
            stale_results: 0,
        }
    }

    fn probe(samples: usize, mut operation: impl FnMut()) -> LatencyPercentiles {
        let mut measurements = Vec::with_capacity(samples);
        for _ in 0..samples {
            let started = Instant::now();
            operation();
            measurements.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        latency_percentiles(&measurements)
    }

    #[test]
    fn percentiles_are_nearest_rank_and_ignore_invalid_samples() {
        let mut samples = (1..=100).map(f64::from).collect::<Vec<_>>();
        samples.extend([f64::NAN, f64::INFINITY, -1.0]);
        let snapshot = latency_percentiles(&samples);
        assert_eq!(snapshot.samples, 100);
        assert_eq!(snapshot.p50_ms, 50.0);
        assert_eq!(snapshot.p95_ms, 95.0);
        assert_eq!(snapshot.p99_ms, 99.0);
    }

    #[test]
    fn unexplained_regressions_fail_but_actionable_explanations_can_waive_them() {
        let mut budgets = ci_budgets().unwrap();
        let mut report = compliant_report(&budgets);
        let metric = MetricName::FilterResponse;
        let budget = budgets
            .metrics
            .iter()
            .find(|candidate| candidate.metric == metric)
            .unwrap();
        let measurement = report
            .metrics
            .iter_mut()
            .find(|candidate| candidate.metric == metric)
            .unwrap();
        measurement.latency.p95_ms =
            budget.baseline_p95_ms * (1.0 + budget.allowed_regression_percent / 100.0) + 0.1;
        measurement.latency.p99_ms = measurement.latency.p95_ms;
        assert!(
            evaluate_performance_gate(&budgets, &report)
                .iter()
                .any(|violation| violation.kind == GateViolationKind::UnexplainedRegression)
        );

        budgets.regression_explanations.push(RegressionExplanation {
            metric,
            owner: "performance".to_string(),
            reason: "Temporary instrumentation cost".to_string(),
            tracking_issue: "#123".to_string(),
            expires_on: "2999-08-01".to_string(),
        });
        assert!(evaluate_performance_gate(&budgets, &report).is_empty());
    }

    #[test]
    fn hard_budget_cannot_be_waived() {
        let mut budgets = ci_budgets().unwrap();
        let mut report = compliant_report(&budgets);
        let metric = MetricName::StartupTotal;
        let hard = budgets
            .metrics
            .iter()
            .find(|candidate| candidate.metric == metric)
            .unwrap()
            .hard_p95_ms;
        let measurement = report
            .metrics
            .iter_mut()
            .find(|candidate| candidate.metric == metric)
            .unwrap();
        measurement.latency.p95_ms = hard + 1.0;
        measurement.latency.p99_ms = hard + 1.0;
        budgets.regression_explanations.push(RegressionExplanation {
            metric,
            owner: "performance".to_string(),
            reason: "Known".to_string(),
            tracking_issue: "#124".to_string(),
            expires_on: "2999-08-01".to_string(),
        });
        assert!(
            evaluate_performance_gate(&budgets, &report)
                .iter()
                .any(|violation| violation.kind == GateViolationKind::HardBudget)
        );
    }

    #[test]
    fn ci_performance_budget_contract_is_complete() {
        let budgets = ci_budgets().unwrap();
        assert_eq!(
            budgets
                .metrics
                .iter()
                .map(|budget| budget.metric)
                .collect::<HashSet<_>>(),
            MetricName::CI.into_iter().collect()
        );
        for budget in budgets.metrics {
            assert!(budget.hard_p95_ms.is_finite() && budget.hard_p95_ms > 0.0);
            assert!(budget.baseline_p95_ms.is_finite() && budget.baseline_p95_ms > 0.0);
            assert!(budget.baseline_p95_ms <= budget.hard_p95_ms);
            assert!((0.0..=100.0).contains(&budget.allowed_regression_percent));
        }
    }

    #[test]
    #[ignore = "timing gate runs in the isolated single-threaded CI step"]
    fn ci_runtime_smoke_probes_stay_within_budgets() {
        let temp = TempDir::new();
        for index in 0..512 {
            temp.file(&format!("file-{index:04}.txt"), "fixture");
        }
        let root = temp.path().to_path_buf();

        let startup = probe(9, || {
            let workspace = crate::workspace::Workspace::with_opener(
                root.clone(),
                root.clone(),
                Box::new(|_| {}),
            );
            std::hint::black_box(workspace.active);
        });
        let first_listing = probe(9, || {
            let mut panel = crate::panel::PanelState::new(root.clone());
            panel.refresh();
            std::hint::black_box(panel.entries().len());
        });
        let mut panel = crate::panel::PanelState::new(root.clone());
        panel.refresh();
        let mut filter_generation = 0_u64;
        let filter_response = probe(31, || {
            filter_generation = filter_generation.saturating_add(1);
            panel.set_search_query(if filter_generation.is_multiple_of(2) {
                "file-1".to_string()
            } else {
                "missing".to_string()
            });
            std::hint::black_box(panel.filtered_count());
        });
        let profile = crate::volume_profile::profile(&root);
        let operation_dialog = probe(31, || {
            let diagnostic =
                crate::capability_diagnostic::for_profile(root.clone(), profile.clone(), true);
            std::hint::black_box(serde_json::to_vec(&diagnostic).unwrap());
        });
        let workload = crate::workload::stats();
        let report = BenchmarkReport {
            schema: 1,
            fixture_profile: "ci-runtime-smoke-v1".to_string(),
            metrics: vec![
                BenchmarkMetric {
                    metric: MetricName::StartupTotal,
                    latency: startup,
                },
                BenchmarkMetric {
                    metric: MetricName::FirstListing,
                    latency: first_listing,
                },
                BenchmarkMetric {
                    metric: MetricName::FilterResponse,
                    latency: filter_response,
                },
                BenchmarkMetric {
                    metric: MetricName::OperationDialog,
                    latency: operation_dialog,
                },
            ],
            cancellation_latency: LatencyPercentiles {
                p50_ms: workload.cancellation_latency_p50_micros as f64 / 1_000.0,
                p95_ms: workload.cancellation_latency_p95_micros as f64 / 1_000.0,
                p99_ms: workload.cancellation_latency_p99_micros as f64 / 1_000.0,
                samples: workload.cancellation_latency_samples,
            },
            stale_results: workload.stale_results,
        };
        let violations = evaluate_performance_gate(&ci_budgets().unwrap(), &report);
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
        assert!(
            violations.is_empty(),
            "{}",
            violations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
