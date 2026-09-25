//! Workload snapshots and regression comparison.
//!
//! Ports `Mxrb::Oql::WorkloadBaseline`: a report serializes to a versioned
//! JSON snapshot, and a later report is compared against it fingerprint by
//! fingerprint. The comparison is deterministic — no clock, no ordering by
//! measurement — so the same pair of snapshots always yields the same deltas.

use serde::Serialize;
use serde_json::Value;

use crate::plan::PlanEngine;
use crate::workload::{WorkloadQuery, WorkloadReport};

/// The only snapshot format this understands. A snapshot from a future
/// version is refused rather than read optimistically.
pub const VERSION: u64 = 1;

/// The metrics compared between two snapshots, in mxrb's order.
pub const METRICS: [BaselineMetric; 5] = [
    BaselineMetric::TotalTimeMs,
    BaselineMetric::MeanTimeMs,
    BaselineMetric::IoTimeMs,
    BaselineMetric::TempWrites,
    BaselineMetric::Rows,
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BaselineError {
    #[error("unsupported workload baseline version")]
    UnsupportedVersion,
    #[error("cannot read workload baseline: {0}")]
    Unreadable(String),
    #[error("workload baseline entry {query_id} has no usable {metric}")]
    MissingMetric {
        query_id: String,
        metric: BaselineMetric,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineMetric {
    TotalTimeMs,
    MeanTimeMs,
    IoTimeMs,
    TempWrites,
    Rows,
}

impl BaselineMetric {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TotalTimeMs => "total_time_ms",
            Self::MeanTimeMs => "mean_time_ms",
            Self::IoTimeMs => "io_time_ms",
            Self::TempWrites => "temp_writes",
            Self::Rows => "rows",
        }
    }

    /// The metric's value on a live query entry, as an `f64` — the same
    /// coercion mxrb's `Float(...)` applies to both sides of a delta.
    #[must_use]
    pub fn of(self, query: &WorkloadQuery) -> f64 {
        match self {
            Self::TotalTimeMs => query.total_time_ms,
            Self::MeanTimeMs => query.mean_time_ms,
            Self::IoTimeMs => query.io_time_ms,
            Self::TempWrites => query.temp_writes as f64,
            Self::Rows => query.rows as f64,
        }
    }
}

impl std::fmt::Display for BaselineMetric {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkloadDelta {
    pub query_id: String,
    pub metric: BaselineMetric,
    pub before: f64,
    pub after: f64,
    /// Positive is a regression: the metric grew. A metric that was zero and
    /// is not any more reports 100%, because a percentage of zero has no
    /// meaning and dropping the delta would hide a new cost.
    pub change_percent: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkloadComparison {
    pub deltas: Vec<WorkloadDelta>,
}

impl WorkloadComparison {
    #[must_use]
    pub fn regressions(&self) -> Vec<&WorkloadDelta> {
        self.deltas
            .iter()
            .filter(|delta| delta.change_percent > 0.0)
            .collect()
    }

    #[must_use]
    pub fn improvements(&self) -> Vec<&WorkloadDelta> {
        self.deltas
            .iter()
            .filter(|delta| delta.change_percent < 0.0)
            .collect()
    }
}

/// Serializes `report` as a snapshot. `captured_at` is supplied by the caller
/// rather than read from a clock here, so the snapshot stays a pure function
/// of its inputs and a test can pin its bytes.
///
/// # Panics
///
/// Never: every field of a [`WorkloadQuery`] is serializable.
#[must_use]
pub fn dump(report: &WorkloadReport, captured_at: &str) -> String {
    let queries = report
        .queries
        .iter()
        .map(|query| {
            (
                query.query_id.clone(),
                serde_json::to_value(query).expect("a workload query is serializable"),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    let payload = serde_json::json!({
        "version": VERSION,
        "engine": report.engine,
        "captured_at": captured_at,
        "queries": queries,
    });
    let mut text = serde_json::to_string_pretty(&payload).expect("a baseline is serializable");
    text.push('\n');
    text
}

/// # Errors
///
/// Returns [`BaselineError::Unreadable`] when `text` is not JSON, and
/// [`BaselineError::UnsupportedVersion`] when it is a snapshot this cannot
/// read.
pub fn load(text: &str) -> Result<Value, BaselineError> {
    let payload: Value =
        serde_json::from_str(text).map_err(|error| BaselineError::Unreadable(error.to_string()))?;
    if payload.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err(BaselineError::UnsupportedVersion);
    }
    Ok(payload)
}

/// Compares `report` against a loaded snapshot. A fingerprint the snapshot
/// never saw contributes nothing: it is new work, not a regression.
///
/// # Errors
///
/// Returns [`BaselineError`] when the snapshot is unreadable, of an unknown
/// version, or carries an entry whose metric is missing or not a number.
/// mxrb raises `KeyError` in that last case; naming the entry and the metric
/// is the same refusal with an actionable message.
pub fn compare(report: &WorkloadReport, text: &str) -> Result<WorkloadComparison, BaselineError> {
    let payload = load(text)?;
    let previous = payload
        .get("queries")
        .and_then(Value::as_object)
        .ok_or_else(|| BaselineError::Unreadable("no queries in the baseline".to_string()))?;
    let mut deltas = Vec::new();
    for query in &report.queries {
        let Some(before) = previous.get(&query.query_id) else {
            continue;
        };
        for metric in METRICS {
            let recorded = before
                .get(metric.as_str())
                .and_then(Value::as_f64)
                .ok_or_else(|| BaselineError::MissingMetric {
                    query_id: query.query_id.clone(),
                    metric,
                })?;
            if let Some(delta) = delta(&query.query_id, metric, recorded, metric.of(query)) {
                deltas.push(delta);
            }
        }
    }
    Ok(WorkloadComparison { deltas })
}

fn delta(query_id: &str, metric: BaselineMetric, before: f64, after: f64) -> Option<WorkloadDelta> {
    if before == 0.0 && after == 0.0 {
        return None;
    }
    let change_percent = if before == 0.0 {
        100.0
    } else {
        round((after - before) * 100.0 / before, 2)
    };
    Some(WorkloadDelta {
        query_id: query_id.to_string(),
        metric,
        before,
        after,
        change_percent,
    })
}

fn round(value: f64, digits: i32) -> f64 {
    let scale = 10_f64.powi(digits);
    (value * scale).round() / scale
}

/// The engine a snapshot was taken from, for callers that want to refuse a
/// cross-engine comparison. mxrb records it and never checks it.
#[must_use]
pub fn engine(payload: &Value) -> Option<PlanEngine> {
    match payload.get("engine").and_then(Value::as_str) {
        Some("postgresql") => Some(PlanEngine::PostgreSql),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::workload::WorkloadAnalyzer;

    fn report(id: &str, total: f64, mean: f64, rows: i64) -> WorkloadReport {
        let row = json!([{
            "queryid": id, "query": "SELECT 1", "calls": "2",
            "total_exec_time": total.to_string(), "mean_exec_time": mean.to_string(),
            "rows": rows.to_string(), "shared_blks_hit": "10", "shared_blks_read": "2",
            "temp_blks_written": "0", "blk_read_time": "1", "blk_write_time": "0"
        }]);
        let rows = row
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row.as_object().unwrap().clone())
            .collect::<Vec<_>>();
        WorkloadAnalyzer::new().analyze(&rows, &[], &[])
    }

    /// Mirrors mxrb's `performance_advisor_spec.rb`: a metric that grew is a
    /// regression, one that shrank is an improvement.
    #[test]
    fn serializes_baselines_and_reports_regressions_and_improvements() {
        let previous = report("q1", 1_000.0, 100.0, 100);
        let current = report("q1", 1_500.0, 80.0, 100);

        let snapshot = dump(&previous, "2026-09-25T00:00:00Z");
        assert!(snapshot.ends_with('\n'));
        let comparison = compare(&current, &snapshot).unwrap();

        let regressions = comparison.regressions();
        assert!(
            regressions
                .iter()
                .any(|delta| delta.metric == BaselineMetric::TotalTimeMs),
            "{regressions:?}"
        );
        let improvements = comparison.improvements();
        assert!(
            improvements
                .iter()
                .any(|delta| delta.metric == BaselineMetric::MeanTimeMs),
            "{improvements:?}"
        );
        let total = comparison
            .deltas
            .iter()
            .find(|delta| delta.metric == BaselineMetric::TotalTimeMs)
            .unwrap();
        assert_eq!(total.before, 1_000.0);
        assert_eq!(total.after, 1_500.0);
        assert_eq!(total.change_percent, 50.0);
        // An unchanged non-zero metric is still reported, at 0% — it is
        // neither a regression nor an improvement, and dropping it would hide
        // that the fingerprint was measured at all. Only a metric that was
        // zero and stayed zero is omitted, which is why `temp_writes` is the
        // one metric missing here.
        let rows = comparison
            .deltas
            .iter()
            .find(|delta| delta.metric == BaselineMetric::Rows)
            .unwrap();
        assert_eq!(rows.change_percent, 0.0);
        assert!(
            !comparison
                .deltas
                .iter()
                .any(|delta| delta.metric == BaselineMetric::TempWrites),
            "{:?}",
            comparison.deltas
        );
    }

    #[test]
    fn rejects_malformed_baselines_and_handles_zero_and_absent_fingerprints() {
        let current = report("new", 0.0, 0.0, 0);
        // A fingerprint the snapshot never saw is new work, not a regression.
        let comparison = compare(&current, r#"{"version": 1, "queries": {}}"#).unwrap();
        assert!(comparison.deltas.is_empty());

        assert_eq!(
            load(r#"{"version": 2}"#),
            Err(BaselineError::UnsupportedVersion)
        );
        assert!(matches!(
            load("not json"),
            Err(BaselineError::Unreadable(_))
        ));
        assert_eq!(
            compare(
                &report("q", 1.0, 1.0, 1),
                r#"{"version": 1, "queries": {"q": {}}}"#
            ),
            Err(BaselineError::MissingMetric {
                query_id: "q".to_string(),
                metric: BaselineMetric::TotalTimeMs
            })
        );
    }

    /// A metric that was zero and is not any more reports 100%: the delta
    /// exists, even though a percentage of zero does not.
    #[test]
    fn a_metric_that_grew_from_zero_is_reported_rather_than_dropped() {
        let current = report("q", 250.0, 10.0, 5);
        let snapshot = dump(&report("q", 0.0, 0.0, 5), "2026-09-25T00:00:00Z");
        let comparison = compare(&current, &snapshot).unwrap();
        let total = comparison
            .deltas
            .iter()
            .find(|delta| delta.metric == BaselineMetric::TotalTimeMs)
            .unwrap();
        assert_eq!(total.before, 0.0);
        assert_eq!(total.change_percent, 100.0);
    }

    #[test]
    fn a_snapshot_records_its_version_engine_and_capture_time() {
        let snapshot = dump(&report("q1", 1.0, 1.0, 1), "2026-09-25T12:00:00Z");
        let payload = load(&snapshot).unwrap();
        assert_eq!(payload["version"], json!(1));
        assert_eq!(payload["engine"], json!("postgresql"));
        assert_eq!(payload["captured_at"], json!("2026-09-25T12:00:00Z"));
        assert_eq!(payload["queries"]["q1"]["total_time_ms"], json!(1.0));
        assert_eq!(engine(&payload), Some(PlanEngine::PostgreSql));
    }
}
