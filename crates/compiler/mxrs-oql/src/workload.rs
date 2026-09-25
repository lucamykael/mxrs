//! Cumulative PostgreSQL workload statistics.
//!
//! Ports `Mxrb::Oql::WorkloadAnalyzer`: rows from `pg_stat_statements`,
//! `pg_stat_user_tables` and `pg_stat_user_indexes` become findings that carry
//! the measured values with them. No threshold here is universally optimal, so
//! a finding reports what it saw and leaves the ranking to a reader who knows
//! their own latency and throughput budget.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::plan::{PlanEngine, Severity};

/// mxrb's `DEFAULT_THRESHOLDS`. A struct rather than a keyword hash: mxrb
/// raises `ArgumentError` on an unknown threshold name, a failure mode that
/// cannot be expressed here at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkloadThresholds {
    pub total_time_ms: f64,
    pub mean_time_ms: f64,
    pub min_buffer_blocks: i64,
    pub min_cache_hit_ratio: f64,
    pub high_rows_per_call: i64,
    pub large_unused_index_bytes: i64,
    pub heavy_seq_rows: i64,
}

impl Default for WorkloadThresholds {
    fn default() -> Self {
        Self {
            total_time_ms: 1_000.0,
            mean_time_ms: 100.0,
            min_buffer_blocks: 100,
            min_cache_hit_ratio: 0.90,
            high_rows_per_call: 10_000,
            large_unused_index_bytes: 1_048_576,
            heavy_seq_rows: 100_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadRule {
    HighCumulativeTime,
    HighMeanTime,
    LowCacheHit,
    TemporaryBlockWrites,
    HighRowsPerCall,
    TableSequentialPressure,
    UnusedLargeIndex,
}

impl WorkloadRule {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HighCumulativeTime => "high_cumulative_time",
            Self::HighMeanTime => "high_mean_time",
            Self::LowCacheHit => "low_cache_hit",
            Self::TemporaryBlockWrites => "temporary_block_writes",
            Self::HighRowsPerCall => "high_rows_per_call",
            Self::TableSequentialPressure => "table_sequential_pressure",
            Self::UnusedLargeIndex => "unused_large_index",
        }
    }
}

impl std::fmt::Display for WorkloadRule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One `pg_stat_statements` fingerprint, normalized. Field order is mxrb's
/// `WorkloadQuery` order, because a baseline dump is keyed by it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkloadQuery {
    pub query_id: String,
    pub query: String,
    pub calls: i64,
    pub total_time_ms: f64,
    pub mean_time_ms: f64,
    pub rows: i64,
    pub shared_hits: i64,
    pub shared_reads: i64,
    pub temp_writes: i64,
    pub io_time_ms: f64,
    pub cache_hit_ratio: f64,
}

/// The measurements behind a finding. The three shapes are distinct on
/// purpose — a table finding has no latency and a query finding has no index
/// size — and serialize untagged, so the payload matches mxrb's plain hash.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WorkloadMetrics {
    Query {
        calls: i64,
        total_time_ms: f64,
        mean_time_ms: f64,
        rows: i64,
        cache_hit_ratio: f64,
        temp_writes: i64,
        io_time_ms: f64,
    },
    Table {
        seq_scan: i64,
        seq_tup_read: i64,
        idx_scan: i64,
        live_rows: i64,
    },
    Index {
        idx_scan: i64,
        index_bytes: i64,
        idx_tup_read: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkloadFinding {
    pub rule: WorkloadRule,
    pub severity: Severity,
    pub subject: String,
    pub message: String,
    pub suggestion: String,
    pub metrics: WorkloadMetrics,
}

/// The analyzed workload. `table_stats` and `index_stats` are the catalog rows
/// exactly as PostgreSQL returned them — mxrb passes them through untouched,
/// and a statistics dump is the one place where reshaping the numbers would
/// lose more than it explains.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkloadReport {
    pub engine: PlanEngine,
    pub queries: Vec<WorkloadQuery>,
    pub table_stats: Vec<Map<String, Value>>,
    pub index_stats: Vec<Map<String, Value>>,
    pub findings: Vec<WorkloadFinding>,
}

impl WorkloadReport {
    /// Every workload finding is a warning, so a clean report is one with no
    /// findings at all. The accessor still exists because the payload names it
    /// and because that equivalence is mxrb's, not an invariant to rely on.
    #[must_use]
    pub fn clean(&self) -> bool {
        !self.warnings()
    }

    #[must_use]
    pub fn warnings(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == Severity::Warning)
    }
}

impl Serialize for WorkloadReport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;

        let mut payload = serializer.serialize_struct("WorkloadReport", 6)?;
        payload.serialize_field("engine", &self.engine)?;
        payload.serialize_field("clean", &self.clean())?;
        payload.serialize_field("queries", &self.queries)?;
        payload.serialize_field("table_stats", &self.table_stats)?;
        payload.serialize_field("index_stats", &self.index_stats)?;
        payload.serialize_field("findings", &self.findings)?;
        payload.end()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WorkloadAnalyzer {
    thresholds: WorkloadThresholds,
}

impl WorkloadAnalyzer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub const fn with_thresholds(thresholds: WorkloadThresholds) -> Self {
        Self { thresholds }
    }

    #[must_use]
    pub fn analyze(
        &self,
        query_rows: &[Map<String, Value>],
        table_rows: &[Map<String, Value>],
        index_rows: &[Map<String, Value>],
    ) -> WorkloadReport {
        let queries: Vec<WorkloadQuery> = query_rows.iter().map(query_entry).collect();
        let findings = queries
            .iter()
            .flat_map(|query| self.query_findings(query))
            .chain(table_rows.iter().filter_map(|row| self.table_finding(row)))
            .chain(index_rows.iter().filter_map(|row| self.index_finding(row)))
            .collect();
        WorkloadReport {
            engine: PlanEngine::PostgreSql,
            queries,
            table_stats: table_rows.to_vec(),
            index_stats: index_rows.to_vec(),
            findings,
        }
    }

    fn query_findings(&self, entry: &WorkloadQuery) -> Vec<WorkloadFinding> {
        [
            self.cumulative_time(entry),
            self.slow_mean(entry),
            self.low_cache_hit(entry),
            self.temporary_writes(entry),
            self.high_rows(entry),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn cumulative_time(&self, entry: &WorkloadQuery) -> Option<WorkloadFinding> {
        (entry.total_time_ms >= self.thresholds.total_time_ms).then(|| {
            query_finding(
                WorkloadRule::HighCumulativeTime,
                entry,
                "This query fingerprint consumes substantial cumulative execution time.",
                "Prioritize it by total time, then inspect its real plan with db explain --analyze.",
            )
        })
    }

    fn slow_mean(&self, entry: &WorkloadQuery) -> Option<WorkloadFinding> {
        (entry.mean_time_ms >= self.thresholds.mean_time_ms).then(|| {
            query_finding(
                WorkloadRule::HighMeanTime,
                entry,
                "Mean execution time exceeds 100 ms.",
                "Inspect plan stability, cardinality estimates, locks, I/O, and returned row volume.",
            )
        })
    }

    fn low_cache_hit(&self, entry: &WorkloadQuery) -> Option<WorkloadFinding> {
        let blocks = entry.shared_hits + entry.shared_reads;
        (blocks >= self.thresholds.min_buffer_blocks
            && entry.cache_hit_ratio < self.thresholds.min_cache_hit_ratio)
            .then(|| {
                query_finding(
                    WorkloadRule::LowCacheHit,
                    entry,
                    "A significant share of shared blocks came from storage.",
                    "Check working-set size, access locality, indexes, and whether the query reads excess rows.",
                )
            })
    }

    fn temporary_writes(&self, entry: &WorkloadQuery) -> Option<WorkloadFinding> {
        (entry.temp_writes > 0).then(|| {
            query_finding(
                WorkloadRule::TemporaryBlockWrites,
                entry,
                "The query wrote temporary blocks.",
                "Inspect sorts and hashes, reduce intermediate rows, and review work_mem per workload.",
            )
        })
    }

    fn high_rows(&self, entry: &WorkloadQuery) -> Option<WorkloadFinding> {
        // Integer division, like mxrb's: the guard is rows *per call*, and a
        // fingerprint with no calls yet has no rate to judge.
        if entry.calls == 0 || entry.rows / entry.calls < self.thresholds.high_rows_per_call {
            return None;
        }
        Some(query_finding(
            WorkloadRule::HighRowsPerCall,
            entry,
            "The query returns or processes many rows per call.",
            "Project only required columns and rows; consider pagination or a narrower aggregation.",
        ))
    }

    fn table_finding(&self, row: &Map<String, Value>) -> Option<WorkloadFinding> {
        let seq_rows = integer(row, "seq_tup_read");
        let seq_scans = integer(row, "seq_scan");
        let index_scans = integer(row, "idx_scan");
        if seq_rows < self.thresholds.heavy_seq_rows || seq_scans <= index_scans {
            return None;
        }
        Some(WorkloadFinding {
            rule: WorkloadRule::TableSequentialPressure,
            severity: Severity::Warning,
            subject: qualified(row, "relname"),
            message: "Cumulative table statistics show more sequential than index scans."
                .to_string(),
            suggestion:
                "Inspect the highest-cost query fingerprints before adding or changing indexes."
                    .to_string(),
            metrics: WorkloadMetrics::Table {
                seq_scan: seq_scans,
                seq_tup_read: seq_rows,
                idx_scan: index_scans,
                live_rows: integer(row, "n_live_tup"),
            },
        })
    }

    fn index_finding(&self, row: &Map<String, Value>) -> Option<WorkloadFinding> {
        if truthy(row, "indisunique") || truthy(row, "indisprimary") {
            return None;
        }
        let size = integer(row, "index_bytes");
        if integer(row, "idx_scan") != 0 || size < self.thresholds.large_unused_index_bytes {
            return None;
        }
        Some(WorkloadFinding {
            rule: WorkloadRule::UnusedLargeIndex,
            severity: Severity::Warning,
            subject: qualified(row, "indexrelname"),
            message: "A non-unique index occupies at least 1 MiB but has no scans in the current statistics window."
                .to_string(),
            suggestion: "Confirm the statistics reset time and production workload before considering removal."
                .to_string(),
            metrics: WorkloadMetrics::Index {
                idx_scan: 0,
                index_bytes: size,
                idx_tup_read: integer(row, "idx_tup_read"),
            },
        })
    }
}

fn query_finding(
    rule: WorkloadRule,
    entry: &WorkloadQuery,
    message: &str,
    suggestion: &str,
) -> WorkloadFinding {
    WorkloadFinding {
        rule,
        severity: Severity::Warning,
        subject: entry.query_id.clone(),
        message: message.to_string(),
        suggestion: suggestion.to_string(),
        metrics: WorkloadMetrics::Query {
            calls: entry.calls,
            total_time_ms: entry.total_time_ms,
            mean_time_ms: entry.mean_time_ms,
            rows: entry.rows,
            cache_hit_ratio: entry.cache_hit_ratio,
            temp_writes: entry.temp_writes,
            io_time_ms: entry.io_time_ms,
        },
    }
}

fn query_entry(row: &Map<String, Value>) -> WorkloadQuery {
    let hits = integer(row, "shared_blks_hit");
    let reads = integer(row, "shared_blks_read");
    let total_blocks = hits + reads;
    // A fingerprint that touched no shared buffer is not a cache miss. mxrb
    // reports a perfect ratio rather than dividing by zero.
    let ratio = if total_blocks == 0 {
        1.0
    } else {
        hits as f64 / total_blocks as f64
    };
    WorkloadQuery {
        query_id: text(row, "queryid"),
        query: text(row, "query"),
        calls: integer(row, "calls"),
        total_time_ms: float(row, "total_exec_time"),
        mean_time_ms: float(row, "mean_exec_time"),
        rows: integer(row, "rows"),
        shared_hits: hits,
        shared_reads: reads,
        temp_writes: integer(row, "temp_blks_written"),
        io_time_ms: float(row, "blk_read_time") + float(row, "blk_write_time"),
        cache_hit_ratio: round(ratio, 4),
    }
}

/// `[schemaname, name].compact.join('.')`: an unqualified row keeps just its
/// name rather than growing a leading dot.
fn qualified(row: &Map<String, Value>, name_key: &str) -> String {
    ["schemaname", name_key]
        .iter()
        .filter_map(|key| row.get(*key).and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(".")
}

/// PostgreSQL renders a boolean catalog column as `t`/`f` through psql, but
/// `true`/`1` reach mxrb from other paths, so all three count.
fn truthy(row: &Map<String, Value>, key: &str) -> bool {
    matches!(
        text(row, key).to_ascii_lowercase().as_str(),
        "t" | "true" | "1"
    )
}

fn text(row: &Map<String, Value>, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// psql hands every column back as CSV text, and mxrb coerces it with Ruby's
/// `String#to_i`/`String#to_f` — a lenient leading-number parse where anything
/// unparseable is zero. These reproduce that rather than failing on a column
/// PostgreSQL rendered in an unexpected form.
fn integer(row: &Map<String, Value>, key: &str) -> i64 {
    match row.get(key) {
        Some(Value::String(text)) => leading_number(text).map_or(0, |value| value.trunc() as i64),
        Some(Value::Number(number)) => number.as_f64().map_or(0, |value| value.trunc() as i64),
        _ => 0,
    }
}

fn float(row: &Map<String, Value>, key: &str) -> f64 {
    match row.get(key) {
        Some(Value::String(text)) => leading_number(text).unwrap_or(0.0),
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// The longest numeric prefix of `text`, or `None` when there is none.
fn leading_number(text: &str) -> Option<f64> {
    let trimmed = text.trim_start();
    let bytes = trimmed.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let digits_start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if end == digits_start {
        return None;
    }
    if bytes.get(end) == Some(&b'.') {
        let fraction = end + 1;
        let mut scan = fraction;
        while bytes.get(scan).is_some_and(u8::is_ascii_digit) {
            scan += 1;
        }
        if scan > fraction {
            end = scan;
        }
    }
    trimmed[..end].parse().ok()
}

/// Ruby's `Float#round(digits)`, which rounds halves away from zero — Rust's
/// `f64::round` agrees, so only the scaling is spelled out here.
fn round(value: f64, digits: i32) -> f64 {
    let scale = 10_f64.powi(digits);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rows(value: Value) -> Vec<Map<String, Value>> {
        value
            .as_array()
            .expect("an array of rows")
            .iter()
            .map(|row| row.as_object().expect("a row object").clone())
            .collect()
    }

    /// Mirrors mxrb's `oql_workload_analyzer_spec.rb` headline case.
    #[test]
    fn ranks_cumulative_time_latency_io_temp_writes_rows_tables_and_indexes() {
        let query_rows = rows(json!([{
            "queryid": "42", "query": "SELECT * FROM sales$order", "calls": "2",
            "total_exec_time": "2400.5", "mean_exec_time": "1200.25", "rows": "40000",
            "shared_blks_hit": "100", "shared_blks_read": "900",
            "temp_blks_written": "12", "blk_read_time": "30", "blk_write_time": "2.5"
        }]));
        let table_rows = rows(json!([{
            "schemaname": "public", "relname": "sales$order", "seq_scan": "50",
            "seq_tup_read": "200000", "idx_scan": "10", "n_live_tup": "50000"
        }]));
        let index_rows = rows(json!([{
            "schemaname": "public", "indexrelname": "sales_order_old_idx",
            "idx_scan": "0", "idx_tup_read": "0", "index_bytes": "2097152",
            "indisunique": "f", "indisprimary": "false"
        }]));

        let report = WorkloadAnalyzer::new().analyze(&query_rows, &table_rows, &index_rows);

        assert_eq!(report.engine, PlanEngine::PostgreSql);
        assert!(!report.clean());
        assert!(report.warnings());
        let query = &report.queries[0];
        assert_eq!(query.query_id, "42");
        assert_eq!(query.calls, 2);
        assert_eq!(query.total_time_ms, 2_400.5);
        assert_eq!(query.mean_time_ms, 1_200.25);
        assert_eq!(query.cache_hit_ratio, 0.1);
        assert_eq!(query.io_time_ms, 32.5);
        assert_eq!(
            report
                .findings
                .iter()
                .map(|finding| finding.rule)
                .collect::<Vec<_>>(),
            [
                WorkloadRule::HighCumulativeTime,
                WorkloadRule::HighMeanTime,
                WorkloadRule::LowCacheHit,
                WorkloadRule::TemporaryBlockWrites,
                WorkloadRule::HighRowsPerCall,
                WorkloadRule::TableSequentialPressure,
                WorkloadRule::UnusedLargeIndex,
            ]
        );
        let subjects = report
            .findings
            .iter()
            .map(|finding| finding.subject.as_str())
            .collect::<Vec<_>>();
        for subject in ["42", "public.sales$order", "public.sales_order_old_idx"] {
            assert!(subjects.contains(&subject), "{subject} missing");
        }
    }

    #[test]
    fn keeps_a_low_cost_workload_clean_and_protects_useful_or_constrained_indexes() {
        let query_rows = rows(json!([
            {
                "queryid": "empty", "query": "SELECT 1", "calls": "0",
                "total_exec_time": "1", "mean_exec_time": "1", "rows": "0",
                "shared_blks_hit": "0", "shared_blks_read": "0",
                "temp_blks_written": "0", "blk_read_time": null, "blk_write_time": null
            },
            {
                "queryid": "cached", "query": "SELECT id FROM product", "calls": "10",
                "total_exec_time": "20", "mean_exec_time": "2", "rows": "100",
                "shared_blks_hit": "99", "shared_blks_read": "1",
                "temp_blks_written": "0", "blk_read_time": "0", "blk_write_time": "0"
            }
        ]));
        let table_rows = rows(json!([{
            "schemaname": "public", "relname": "product", "seq_scan": "1",
            "seq_tup_read": "5", "idx_scan": "10", "n_live_tup": "5"
        }]));
        let index_rows = rows(json!([
            {"index_bytes": "999", "idx_scan": "0", "indisunique": "f"},
            {"index_bytes": "2000000", "idx_scan": "1", "indisunique": "f"},
            {"index_bytes": "2000000", "idx_scan": "0", "indisunique": "true"},
            {"index_bytes": "2000000", "idx_scan": "0", "indisprimary": "1"}
        ]));

        let report = WorkloadAnalyzer::new().analyze(&query_rows, &table_rows, &index_rows);

        assert!(report.clean());
        assert!(!report.warnings());
        assert!(report.findings.is_empty());
        // No shared buffer touched is a perfect ratio, not a division by zero.
        assert_eq!(report.queries[0].cache_hit_ratio, 1.0);
        // A NULL I/O timing column reads as zero, the way `nil.to_f` does.
        assert_eq!(report.queries[0].io_time_ms, 0.0);
    }

    /// mxrb's thresholds are a keyword hash it has to validate; here they are a
    /// struct, so an unknown threshold cannot be written in the first place.
    #[test]
    fn thresholds_are_configurable_and_every_field_is_named() {
        let query_rows = rows(json!([{
            "queryid": "q", "query": "SELECT 1", "calls": "1",
            "total_exec_time": "10", "mean_exec_time": "10", "rows": "1",
            "shared_blks_hit": "1", "shared_blks_read": "0",
            "temp_blks_written": "0", "blk_read_time": "0", "blk_write_time": "0"
        }]));
        let analyzer = WorkloadAnalyzer::with_thresholds(WorkloadThresholds {
            total_time_ms: 5.0,
            mean_time_ms: 5.0,
            ..WorkloadThresholds::default()
        });
        assert_eq!(
            analyzer
                .analyze(&query_rows, &[], &[])
                .findings
                .iter()
                .map(|finding| finding.rule)
                .collect::<Vec<_>>(),
            [WorkloadRule::HighCumulativeTime, WorkloadRule::HighMeanTime]
        );
        // The defaults leave the same workload clean.
        assert!(
            WorkloadAnalyzer::new()
                .analyze(&query_rows, &[], &[])
                .findings
                .is_empty()
        );
    }

    /// The `--json` payload is the contract `mxrs db workload --json` prints:
    /// mxrb's key order, `clean` derived, statistics rows passed through.
    #[test]
    fn the_json_payload_keeps_the_raw_statistics_rows_and_derives_clean() {
        let table_rows = rows(json!([{
            "schemaname": "public", "relname": "product", "seq_scan": "1",
            "seq_tup_read": "5", "idx_scan": "10", "n_live_tup": "5",
            "last_analyze": null
        }]));
        let report = WorkloadAnalyzer::new().analyze(&[], &table_rows, &[]);
        let rendered = serde_json::to_value(&report).unwrap();
        assert_eq!(rendered["engine"], json!("postgresql"));
        assert_eq!(rendered["clean"], json!(true));
        assert_eq!(rendered["queries"], json!([]));
        assert_eq!(rendered["table_stats"][0]["last_analyze"], Value::Null);
        assert_eq!(rendered["findings"], json!([]));
        // mxrb's key order, so a payload from either tool reads the same.
        let text = serde_json::to_string(&report).unwrap();
        let positions = [
            "engine",
            "clean",
            "queries",
            "table_stats",
            "index_stats",
            "findings",
        ]
        .map(|key| text.find(&format!("\"{key}\"")).expect(key));
        assert!(positions.is_sorted(), "{text}");
    }

    /// A finding's metrics carry the shape of what was measured, and nothing
    /// else: a table finding has no latency, an index finding has no rows.
    #[test]
    fn finding_metrics_serialize_untagged_in_their_own_shape() {
        let table_rows = rows(json!([{
            "schemaname": "public", "relname": "orders", "seq_scan": "50",
            "seq_tup_read": "200000", "idx_scan": "10", "n_live_tup": "50000"
        }]));
        let index_rows = rows(json!([{
            "schemaname": "public", "indexrelname": "orders_old_idx", "idx_scan": "0",
            "idx_tup_read": "7", "index_bytes": "2097152", "indisunique": "f"
        }]));
        let report = WorkloadAnalyzer::new().analyze(&[], &table_rows, &index_rows);
        let rendered = serde_json::to_value(&report).unwrap();
        assert_eq!(
            rendered["findings"][0]["metrics"],
            json!({"seq_scan": 50, "seq_tup_read": 200_000, "idx_scan": 10, "live_rows": 50_000})
        );
        assert_eq!(
            rendered["findings"][1]["metrics"],
            json!({"idx_scan": 0, "index_bytes": 2_097_152, "idx_tup_read": 7})
        );
        assert_eq!(
            rendered["findings"][1]["subject"],
            json!("public.orders_old_idx")
        );
    }

    #[test]
    fn a_row_without_a_schema_keeps_just_its_name() {
        let index_rows = rows(json!([{
            "indexrelname": "orphan_idx", "idx_scan": "0", "index_bytes": "2097152"
        }]));
        let report = WorkloadAnalyzer::new().analyze(&[], &[], &index_rows);
        assert_eq!(report.findings[0].subject, "orphan_idx");
    }
}
