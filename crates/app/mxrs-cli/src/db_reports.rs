//! Human-readable rendering of the database command's analysis reports.
//!
//! These live in the library rather than beside the dispatcher so the text a
//! user actually reads is pinned by tests. Each renderer returns a `String`
//! instead of printing: the shape is the contract, and a function that writes
//! to stdout cannot be asserted on.

use mxrs_oql::baseline::WorkloadComparison;
use mxrs_oql::index_advisor::IndexAdvice;
use mxrs_oql::plan::PlanReport;
use mxrs_oql::workload::WorkloadReport;

/// Renders a query plan the way mxrb's `render_plan_report` does.
///
/// A finding never changes the exit status of the command that produced it —
/// a plan diagnosis is advice about a query that ran, not a failure.
#[must_use]
pub fn render_plan_report(report: &PlanReport) -> String {
    let mut text = format!(
        "Engine         : {}\nMode           : {}\nTotal cost     : {}\n",
        report.engine,
        if report.analyzed {
            "actual"
        } else {
            "estimated"
        },
        report
            .total_cost
            .as_ref()
            .map_or_else(String::new, ToString::to_string)
    );
    for finding in &report.findings {
        // mxrb's `finding.relation || finding.node_type`, including the case
        // where a node has neither and the subject comes out empty.
        let subject = finding
            .relation
            .as_deref()
            .or(finding.node_type.as_deref())
            .unwrap_or_default();
        text.push_str(&format!(
            "[{}] {}: {subject}\n  {}\n  {}\n",
            finding.severity.as_str().to_uppercase(),
            finding.rule,
            finding.message,
            finding.suggestion
        ));
    }
    if report.findings.is_empty() {
        text.push_str("[mxrs] No plan findings\n");
    }
    text
}

/// Renders cumulative workload statistics the way mxrb's
/// `render_workload_report` does: one line per fingerprint, ranked by total
/// time, then one block per finding.
#[must_use]
pub fn render_workload_report(report: &WorkloadReport) -> String {
    let mut text = String::new();
    for query in &report.queries {
        text.push_str(&format!(
            "{}\t{} calls\t{} ms total\t{} ms mean\n",
            query.query_id,
            query.calls,
            decimal(query.total_time_ms, 3),
            decimal(query.mean_time_ms, 3)
        ));
    }
    for finding in &report.findings {
        text.push_str(&format!(
            "[{}] {}: {}\n  {}\n  {}\n",
            finding.severity.as_str().to_uppercase(),
            finding.rule,
            finding.subject,
            finding.message,
            finding.suggestion
        ));
    }
    if report.findings.is_empty() {
        text.push_str("[mxrs] No workload findings\n");
    }
    text
}

/// The deltas against a saved baseline, appended after the report they belong
/// to. A delta is not a finding: it says a number moved, not that the number
/// is wrong.
#[must_use]
pub fn render_comparison(comparison: &WorkloadComparison) -> String {
    comparison
        .deltas
        .iter()
        .map(|delta| {
            format!(
                "[DELTA] {} {}: {}%\n",
                delta.query_id,
                delta.metric,
                decimal(delta.change_percent, 2)
            )
        })
        .collect()
}

/// Renders index advice the way mxrb's `render_index_advice` does. A candidate
/// is a hypothesis to test, which is why its supporting fingerprints are
/// printed beside it rather than summarized away.
#[must_use]
pub fn render_index_advice(advice: &IndexAdvice) -> String {
    let mut text = String::new();
    for candidate in &advice.candidates {
        text.push_str(&format!(
            "[{}] {}({})\n  {} Queries: {}\n",
            candidate.confidence.as_str().to_uppercase(),
            candidate.relation,
            candidate.columns.join(", "),
            candidate.reason,
            candidate.query_ids.join(", ")
        ));
    }
    for pair in &advice.redundant_indexes {
        text.push_str(&format!(
            "[WARNING] overlapping indexes: {} and {}\n",
            pair.0, pair.1
        ));
    }
    if advice.candidates.is_empty() {
        text.push_str("[mxrs] No evidence-backed index candidates\n");
    }
    text
}

/// A rounded float printed the way Ruby prints one: a whole value keeps its
/// `.0`, so `20` and `20.0` cannot be confused for different measurements
/// across the two tools.
fn decimal(value: f64, digits: i32) -> String {
    let scale = 10_f64.powi(digits);
    let rounded = (value * scale).round() / scale;
    if rounded.fract() == 0.0 && rounded.is_finite() {
        format!("{rounded:.1}")
    } else {
        format!("{rounded}")
    }
}

/// The capture time of a workload baseline, at second precision like Ruby's
/// `Time#iso8601`.
///
/// # Panics
///
/// Never: the clock is always representable as RFC 3339.
#[must_use]
pub fn captured_at() -> String {
    let stamp = mxrs_bson::DateTime::now()
        .try_to_rfc3339_string()
        .expect("the current time is representable");
    match stamp.split_once('.') {
        Some((seconds, _)) => format!("{seconds}Z"),
        None => stamp,
    }
}

#[cfg(test)]
mod tests {
    use mxrs_oql::plan::PlanAnalyzer;
    use serde_json::{Value, json};

    use super::*;

    fn report(plan: Value, analyzed: bool) -> PlanReport {
        PlanAnalyzer::default()
            .analyze(&json!([{"Plan": plan}]), analyzed)
            .expect("a well-formed plan envelope")
    }

    #[test]
    fn a_plan_with_findings_names_the_relation_severity_and_both_sentences() {
        let rendered = render_plan_report(&report(
            json!({
                "Node Type": "Seq Scan", "Schema": "public", "Relation Name": "orders",
                "Plan Rows": 5_000, "Total Cost": 359.0, "Filter": "(status = 'Open')"
            }),
            true,
        ));
        assert_eq!(
            rendered,
            "Engine         : postgresql\n\
             Mode           : actual\n\
             Total cost     : 359.0\n\
             [WARNING] sequential_scan: public.orders\n  \
             A large sequential scan may dominate this query.\n  \
             Review the filter \"(status = 'Open')\". \
             No existing index was found for this relation.\n"
        );
    }

    #[test]
    fn a_clean_plan_says_so_rather_than_printing_nothing() {
        let rendered = render_plan_report(&report(json!({"Node Type": "Result"}), false));
        assert_eq!(
            rendered,
            "Engine         : postgresql\n\
             Mode           : estimated\n\
             Total cost     : \n\
             [mxrs] No plan findings\n"
        );
    }

    /// A hint is rendered like a warning but keeps its own label, so a small
    /// scan cannot be mistaken for something that costs the query.
    #[test]
    fn a_hint_keeps_its_own_severity_label() {
        let rendered = render_plan_report(&report(
            json!({"Node Type": "Seq Scan", "Relation Name": "tiny", "Plan Rows": 4}),
            false,
        ));
        assert!(
            rendered.contains("[HINT] sequential_scan: tiny\n"),
            "{rendered}"
        );
        assert!(rendered.contains("No change is implied"), "{rendered}");
    }

    fn workload(rows: Value) -> WorkloadReport {
        let rows = rows
            .as_array()
            .expect("an array of rows")
            .iter()
            .map(|row| row.as_object().expect("a row object").clone())
            .collect::<Vec<_>>();
        mxrs_oql::workload::WorkloadAnalyzer::new().analyze(&rows, &[], &[])
    }

    /// One line per fingerprint, and mxrb's number formatting: a whole value
    /// keeps its `.0` so the two tools cannot disagree about what was measured.
    #[test]
    fn a_workload_lists_each_fingerprint_then_its_findings() {
        let rendered = render_workload_report(&workload(json!([{
            "queryid": "42", "query": "SELECT * FROM orders", "calls": "2",
            "total_exec_time": "2400.5", "mean_exec_time": "1200", "rows": "10",
            "shared_blks_hit": "100", "shared_blks_read": "900",
            "temp_blks_written": "0", "blk_read_time": "0", "blk_write_time": "0"
        }])));
        assert_eq!(
            rendered,
            "42\t2 calls\t2400.5 ms total\t1200.0 ms mean\n\
             [WARNING] high_cumulative_time: 42\n  \
             This query fingerprint consumes substantial cumulative execution time.\n  \
             Prioritize it by total time, then inspect its real plan with db explain --analyze.\n\
             [WARNING] high_mean_time: 42\n  \
             Mean execution time exceeds 100 ms.\n  \
             Inspect plan stability, cardinality estimates, locks, I/O, and returned row volume.\n\
             [WARNING] low_cache_hit: 42\n  \
             A significant share of shared blocks came from storage.\n  \
             Check working-set size, access locality, indexes, and whether the query reads excess rows.\n"
        );
    }

    #[test]
    fn a_quiet_workload_and_an_unsupported_index_guess_both_say_so() {
        assert_eq!(
            render_workload_report(&workload(json!([]))),
            "[mxrs] No workload findings\n"
        );
        assert_eq!(
            render_index_advice(&IndexAdvice {
                candidates: Vec::new(),
                redundant_indexes: Vec::new(),
            }),
            "[mxrs] No evidence-backed index candidates\n"
        );
    }

    #[test]
    fn index_advice_prints_its_supporting_fingerprints_and_overlapping_pairs() {
        let advice = IndexAdvice {
            candidates: vec![mxrs_oql::index_advisor::IndexCandidate {
                relation: "orders".to_string(),
                columns: vec!["status".to_string(), "id".to_string()],
                confidence: mxrs_oql::index_advisor::Confidence::High,
                query_ids: vec!["q1".to_string(), "q2".to_string()],
                reason: "Because.",
            }],
            redundant_indexes: vec![mxrs_oql::index_advisor::RedundantPair(
                "a".to_string(),
                "b".to_string(),
            )],
        };
        assert_eq!(
            render_index_advice(&advice),
            "[HIGH] orders(status, id)\n  \
             Because. Queries: q1, q2\n\
             [WARNING] overlapping indexes: a and b\n"
        );
    }

    /// `--compare`'s entire output is these lines, so their shape is the
    /// contract. A delta is reported even at 0%: the metric was measured.
    #[test]
    fn a_comparison_prints_one_delta_line_per_metric_that_moved() {
        let previous = workload(json!([{
            "queryid": "q1", "query": "SELECT 1", "calls": "2",
            "total_exec_time": "1000", "mean_exec_time": "100", "rows": "100",
            "shared_blks_hit": "10", "shared_blks_read": "2",
            "temp_blks_written": "0", "blk_read_time": "1", "blk_write_time": "0"
        }]));
        let current = workload(json!([{
            "queryid": "q1", "query": "SELECT 1", "calls": "2",
            "total_exec_time": "1500", "mean_exec_time": "80", "rows": "100",
            "shared_blks_hit": "10", "shared_blks_read": "2",
            "temp_blks_written": "0", "blk_read_time": "1", "blk_write_time": "0"
        }]));
        let comparison = mxrs_oql::baseline::compare(
            &current,
            &mxrs_oql::baseline::dump(&previous, "2026-09-25T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(
            render_comparison(&comparison),
            "[DELTA] q1 total_time_ms: 50.0%\n\
             [DELTA] q1 mean_time_ms: -20.0%\n\
             [DELTA] q1 io_time_ms: 0.0%\n\
             [DELTA] q1 rows: 0.0%\n"
        );
        assert!(
            render_comparison(&mxrs_oql::baseline::WorkloadComparison { deltas: Vec::new() })
                .is_empty()
        );
    }

    /// A capture stamp is second-precision RFC 3339, the shape Ruby's
    /// `Time#iso8601` writes into a baseline.
    #[test]
    fn a_capture_stamp_is_second_precision_and_zulu() {
        let stamp = captured_at();
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert_eq!(&stamp[10..11], "T", "{stamp}");
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert!(!stamp.contains('.'), "{stamp}");
    }

    /// A node with neither a relation nor a node type is the one case where
    /// mxrb prints an empty subject. Matching it keeps the two renderings
    /// comparable line for line.
    #[test]
    fn a_node_with_no_name_renders_an_empty_subject_like_mxrb() {
        let rendered = render_plan_report(&report(
            json!({"Plans": [{"Rows Removed by Filter": 2_000, "Actual Rows": 1}]}),
            false,
        ));
        assert!(
            rendered.contains("[WARNING] filter_discard: \n"),
            "{rendered}"
        );
    }
}
