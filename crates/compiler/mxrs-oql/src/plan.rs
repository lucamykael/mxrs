//! PostgreSQL query-plan diagnostics.
//!
//! Ports `Mxrb::Oql::PlanAnalyzer`: `EXPLAIN (FORMAT JSON)` output becomes a
//! small set of conservative, actionable findings. A sequential scan is not
//! automatically an error — scanning a small relation is often cheaper than an
//! index lookup — so estimated and measured rows decide whether a scan is a
//! hint or a warning. Nothing here guesses at index columns: a finding names
//! the relation, the filter and the indexes that already exist, and leaves the
//! recommendation to the reader.

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

/// Row counts at or above this are "large" for a scan, and discarded-row
/// volume at or above it is worth reporting.
const LARGE_ROWS: f64 = 1_000.0;
/// A plan node costing at least this much is large regardless of row counts.
const HIGH_COST: f64 = 1_000.0;
/// Estimated and actual rows this far apart mean the planner was misled.
const MISESTIMATE_RATIO: f64 = 10.0;
/// Below this many rows a misestimate is noise rather than a planning problem.
const MISESTIMATE_FLOOR: f64 = 100.0;
/// Rows processed by a nested loop before its join strategy is worth a look.
const NESTED_LOOP_ROWS: f64 = 10_000.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("invalid PostgreSQL EXPLAIN JSON")]
    InvalidPayload,
    #[error("PostgreSQL EXPLAIN JSON has no Plan")]
    MissingPlan,
}

/// The planner whose output was analyzed. Only PostgreSQL is ported; mxrb's
/// SQL Server analyzer is a separate engine with a different plan shape, and
/// naming the engine in the report keeps that distinction visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PlanEngine {
    #[serde(rename = "postgresql")]
    PostgreSql,
}

impl PlanEngine {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PostgreSql => "postgresql",
        }
    }
}

impl std::fmt::Display for PlanEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanRule {
    SequentialScan,
    FilterDiscard,
    CardinalityMisestimation,
    HighVolumeNestedLoop,
    DiskSort,
}

impl PlanRule {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SequentialScan => "sequential_scan",
            Self::FilterDiscard => "filter_discard",
            Self::CardinalityMisestimation => "cardinality_misestimation",
            Self::HighVolumeNestedLoop => "high_volume_nested_loop",
            Self::DiskSort => "disk_sort",
        }
    }
}

impl std::fmt::Display for PlanRule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A hint describes a plan choice that may well be optimal; a warning names
/// something that measurably costs the query. Only warnings make a report
/// unclean, so a small-table scan never turns a clean plan into a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Hint,
    Warning,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hint => "hint",
            Self::Warning => "warning",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One row of `pg_indexes`, the catalog a finding is matched against.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IndexCatalogEntry {
    pub schemaname: Option<String>,
    pub tablename: String,
    pub indexname: String,
    pub indexdef: String,
}

/// An existing index on the relation a finding points at. The definition
/// travels with the name so a reader can compare predicates without a second
/// catalog query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexReference {
    pub name: String,
    pub definition: String,
}

/// The planner numbers carried by a finding, in the order mxrb emits them.
/// Absent keys are omitted rather than rendered as null: a plan without
/// `ANALYZE` has no measured rows at all, and a zero would be a lie.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PlanMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_rows: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_rows: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_loops: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_removed_by_filter: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_cost: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_total_time: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_space_used: Option<Number>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_space_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanFinding {
    pub rule: PlanRule,
    pub severity: Severity,
    pub node_type: Option<String>,
    pub relation: Option<String>,
    pub message: String,
    pub suggestion: String,
    pub metrics: PlanMetrics,
    pub indexes: Vec<IndexReference>,
}

/// The analyzed plan. Serializes as mxrb's `db explain --json` payload:
/// `clean` is derived from the findings rather than stored, so the two can
/// never disagree, and `raw` stays out of the payload the way mxrb's does.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanReport {
    pub engine: PlanEngine,
    pub analyzed: bool,
    pub planning_time_ms: Option<Number>,
    pub execution_time_ms: Option<Number>,
    pub total_cost: Option<Number>,
    pub findings: Vec<PlanFinding>,
    /// The `EXPLAIN` payload exactly as PostgreSQL returned it.
    pub raw: Value,
}

impl PlanReport {
    /// A plan with no warnings. Hints do not make a plan unclean.
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

impl Serialize for PlanReport {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;

        let mut payload = serializer.serialize_struct("PlanReport", 7)?;
        payload.serialize_field("engine", &self.engine)?;
        payload.serialize_field("analyzed", &self.analyzed)?;
        payload.serialize_field("total_cost", &self.total_cost)?;
        payload.serialize_field("planning_time_ms", &self.planning_time_ms)?;
        payload.serialize_field("execution_time_ms", &self.execution_time_ms)?;
        payload.serialize_field("clean", &self.clean())?;
        payload.serialize_field("findings", &self.findings)?;
        payload.end()
    }
}

/// Turns PostgreSQL `EXPLAIN` JSON into conservative diagnostics, matched
/// against the index catalog the workspace already has.
#[derive(Debug, Clone, Default)]
pub struct PlanAnalyzer {
    indexes: Vec<IndexCatalogEntry>,
}

impl PlanAnalyzer {
    #[must_use]
    pub fn new(indexes: Vec<IndexCatalogEntry>) -> Self {
        Self { indexes }
    }

    /// # Errors
    ///
    /// Returns [`PlanError`] when the payload is not PostgreSQL's
    /// `EXPLAIN (FORMAT JSON)` envelope, or carries no `Plan`.
    pub fn analyze(&self, payload: &Value, analyzed: bool) -> Result<PlanReport, PlanError> {
        let envelope = payload
            .as_array()
            .and_then(|envelopes| envelopes.first())
            .filter(|envelope| envelope.is_object())
            .ok_or(PlanError::InvalidPayload)?;
        let root = envelope
            .get("Plan")
            .filter(|plan| plan.is_object())
            .ok_or(PlanError::MissingPlan)?;
        let mut nodes = Vec::new();
        walk(root, &mut nodes);
        let findings = nodes
            .into_iter()
            .flat_map(|node| self.findings_for(node))
            .collect();
        Ok(PlanReport {
            engine: PlanEngine::PostgreSql,
            analyzed,
            planning_time_ms: number_field(envelope, "Planning Time"),
            execution_time_ms: number_field(envelope, "Execution Time"),
            total_cost: number_field(root, "Total Cost"),
            findings,
            raw: payload.clone(),
        })
    }

    fn findings_for(&self, node: &Value) -> Vec<PlanFinding> {
        [
            self.sequential_scan(node),
            self.filter_discard(node),
            self.cardinality_misestimation(node),
            self.nested_loop(node),
            self.disk_sort(node),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn sequential_scan(&self, node: &Value) -> Option<PlanFinding> {
        if text(node, "Node Type").as_deref() != Some("Seq Scan") {
            return None;
        }
        let large = relevant_rows(node) >= LARGE_ROWS || number(node, "Total Cost") >= HIGH_COST;
        let indexes = self.indexes_for(node);
        Some(PlanFinding {
            rule: PlanRule::SequentialScan,
            severity: if large {
                Severity::Warning
            } else {
                Severity::Hint
            },
            node_type: Some("Seq Scan".to_string()),
            relation: qualified_relation(node),
            message: if large {
                "A large sequential scan may dominate this query.".to_string()
            } else {
                "A sequential scan was chosen; it may be optimal for a small relation.".to_string()
            },
            suggestion: scan_suggestion(node, &indexes, large),
            metrics: node_metrics(node),
            indexes,
        })
    }

    fn filter_discard(&self, node: &Value) -> Option<PlanFinding> {
        let removed = number(node, "Rows Removed by Filter");
        if removed < LARGE_ROWS || removed <= number(node, "Actual Rows") {
            return None;
        }
        Some(self.warning(
            PlanRule::FilterDiscard,
            node,
            "The filter discarded more rows than it returned.",
            "Review predicate selectivity and whether a matching index can filter before heap access.",
        ))
    }

    fn cardinality_misestimation(&self, node: &Value) -> Option<PlanFinding> {
        let estimated = number(node, "Plan Rows");
        let actual = number(node, "Actual Rows");
        if estimated == 0.0 || actual == 0.0 {
            return None;
        }
        if estimated.max(actual) < MISESTIMATE_FLOOR || ratio(estimated, actual) < MISESTIMATE_RATIO
        {
            return None;
        }
        Some(self.warning(
            PlanRule::CardinalityMisestimation,
            node,
            "Estimated and actual row counts differ by at least 10x.",
            "Run ANALYZE after representative data changes and review extended statistics for correlated columns.",
        ))
    }

    fn nested_loop(&self, node: &Value) -> Option<PlanFinding> {
        if text(node, "Node Type").as_deref() != Some("Nested Loop") {
            return None;
        }
        let rows = relevant_rows(node) * number(node, "Actual Loops").max(1.0);
        if rows < NESTED_LOOP_ROWS {
            return None;
        }
        Some(self.warning(
            PlanRule::HighVolumeNestedLoop,
            node,
            "A nested loop processes a high estimated or measured row volume.",
            "Check join cardinality and indexes on the inner join keys; compare hash or merge join plans.",
        ))
    }

    fn disk_sort(&self, node: &Value) -> Option<PlanFinding> {
        let spilled = text(node, "Sort Space Type").as_deref() == Some("Disk")
            || text(node, "Sort Method").is_some_and(|method| method.starts_with("external"));
        if text(node, "Node Type").as_deref() != Some("Sort") || !spilled {
            return None;
        }
        Some(self.warning(
            PlanRule::DiskSort,
            node,
            "The sort spilled to disk.",
            "Reduce sorted rows, use an order-compatible index, or review work_mem for this workload.",
        ))
    }

    fn warning(
        &self,
        rule: PlanRule,
        node: &Value,
        message: &str,
        suggestion: &str,
    ) -> PlanFinding {
        PlanFinding {
            rule,
            severity: Severity::Warning,
            node_type: text(node, "Node Type"),
            relation: qualified_relation(node),
            message: message.to_string(),
            suggestion: suggestion.to_string(),
            metrics: node_metrics(node),
            indexes: self.indexes_for(node),
        }
    }

    fn indexes_for(&self, node: &Value) -> Vec<IndexReference> {
        let Some(relation) = text(node, "Relation Name") else {
            return Vec::new();
        };
        let schema = text(node, "Schema");
        self.indexes
            .iter()
            .filter(|index| index.tablename == relation)
            .filter(|index| {
                schema
                    .as_deref()
                    .is_none_or(|schema| index.schemaname.as_deref() == Some(schema))
            })
            .map(|index| IndexReference {
                name: index.indexname.clone(),
                definition: index.indexdef.clone(),
            })
            .collect()
    }
}

fn scan_suggestion(node: &Value, indexes: &[IndexReference], large: bool) -> String {
    if !large {
        return "No change is implied; compare with an index plan as data grows.".to_string();
    }
    // The filter is quoted the way mxrb's `String#inspect` quotes it, so the
    // two tools print the same predicate for the same plan.
    let prefix = text(node, "Filter").map_or_else(
        || "Review selective predicates. ".to_string(),
        |filter| format!("Review the filter {filter:?}. "),
    );
    let suffix = if indexes.is_empty() {
        "No existing index was found for this relation."
    } else {
        "Compare predicates with the listed existing indexes."
    };
    prefix + suffix
}

fn walk<'a>(node: &'a Value, nodes: &mut Vec<&'a Value>) {
    nodes.push(node);
    if let Some(children) = node.get("Plans").and_then(Value::as_array) {
        for child in children {
            walk(child, nodes);
        }
    }
}

fn node_metrics(node: &Value) -> PlanMetrics {
    PlanMetrics {
        plan_rows: number_field(node, "Plan Rows"),
        actual_rows: number_field(node, "Actual Rows"),
        actual_loops: number_field(node, "Actual Loops"),
        rows_removed_by_filter: number_field(node, "Rows Removed by Filter"),
        startup_cost: number_field(node, "Startup Cost"),
        total_cost: number_field(node, "Total Cost"),
        actual_total_time: number_field(node, "Actual Total Time"),
        sort_space_used: number_field(node, "Sort Space Used"),
        sort_space_type: text(node, "Sort Space Type"),
    }
}

fn relevant_rows(node: &Value) -> f64 {
    number(node, "Plan Rows").max(number(node, "Actual Rows"))
}

fn ratio(left: f64, right: f64) -> f64 {
    left.max(right) / left.min(right)
}

/// PostgreSQL types every plan metric as a JSON number, so anything else is
/// treated as absent rather than coerced.
fn number(node: &Value, key: &str) -> f64 {
    number_field(node, key)
        .as_ref()
        .and_then(Number::as_f64)
        .unwrap_or(0.0)
}

fn number_field(node: &Value, key: &str) -> Option<Number> {
    match node.get(key) {
        Some(Value::Number(value)) => Some(value.clone()),
        _ => None,
    }
}

fn text(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn qualified_relation(node: &Value) -> Option<String> {
    let relation = text(node, "Relation Name")?;
    Some(match text(node, "Schema") {
        Some(schema) => format!("{schema}.{relation}"),
        None => relation,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn envelope(plan: Value, timings: Value) -> Value {
        let mut first = timings;
        first["Plan"] = plan;
        json!([first])
    }

    fn catalog() -> Vec<IndexCatalogEntry> {
        vec![
            IndexCatalogEntry {
                schemaname: Some("public".to_string()),
                tablename: "sales$order".to_string(),
                indexname: "sales_order_status_idx".to_string(),
                indexdef: "CREATE INDEX sales_order_status_idx ON public.sales$order(status)"
                    .to_string(),
            },
            IndexCatalogEntry {
                schemaname: Some("archive".to_string()),
                tablename: "sales$order".to_string(),
                indexname: "archive_idx".to_string(),
                indexdef: "CREATE INDEX archive_idx ...".to_string(),
            },
            IndexCatalogEntry {
                schemaname: Some("public".to_string()),
                tablename: "sales$customer".to_string(),
                indexname: "customer_idx".to_string(),
                indexdef: "CREATE INDEX customer_idx ...".to_string(),
            },
        ]
    }

    /// Mirrors mxrb's `oql_plan_analyzer_spec.rb` headline case: every rule
    /// fires once, and the scan is matched only against indexes in its own
    /// schema.
    #[test]
    fn finds_costly_scans_discarded_rows_misestimates_nested_loops_and_disk_sorts() {
        let plan = json!({
            "Node Type": "Nested Loop", "Plan Rows": 20_000, "Actual Rows": 3_000,
            "Actual Loops": 20, "Total Cost": 4_000.5,
            "Plans": [
                {
                    "Node Type": "Seq Scan", "Schema": "public", "Relation Name": "sales$order",
                    "Plan Rows": 100, "Actual Rows": 2_000, "Actual Loops": 1,
                    "Rows Removed by Filter": 5_000, "Total Cost": 2_000,
                    "Filter": "(status = 'Open')"
                },
                {
                    "Node Type": "Sort", "Plan Rows": 50, "Actual Rows": 50,
                    "Sort Space Type": "Disk", "Sort Space Used": 2_048,
                    "Sort Method": "quicksort"
                }
            ]
        });
        let payload = envelope(plan, json!({"Planning Time": 1.25, "Execution Time": 20.5}));

        let report = PlanAnalyzer::new(catalog())
            .analyze(&payload, true)
            .unwrap();

        assert_eq!(report.engine, PlanEngine::PostgreSql);
        assert!(report.analyzed);
        assert_eq!(
            report.planning_time_ms,
            Some(Number::from_f64(1.25).unwrap())
        );
        assert_eq!(
            report.execution_time_ms,
            Some(Number::from_f64(20.5).unwrap())
        );
        assert_eq!(report.total_cost, Some(Number::from_f64(4_000.5).unwrap()));
        assert!(!report.clean());
        assert!(report.warnings());
        let mut rules = report
            .findings
            .iter()
            .map(|finding| finding.rule)
            .collect::<Vec<_>>();
        rules.sort_by_key(|rule| rule.as_str());
        assert_eq!(
            rules,
            [
                PlanRule::CardinalityMisestimation,
                PlanRule::DiskSort,
                PlanRule::FilterDiscard,
                PlanRule::HighVolumeNestedLoop,
                PlanRule::SequentialScan,
            ]
        );
        let scan = report
            .findings
            .iter()
            .find(|finding| finding.rule == PlanRule::SequentialScan)
            .unwrap();
        assert_eq!(scan.severity, Severity::Warning);
        assert_eq!(scan.relation.as_deref(), Some("public.sales$order"));
        assert_eq!(
            scan.indexes
                .iter()
                .map(|index| index.name.as_str())
                .collect::<Vec<_>>(),
            ["sales_order_status_idx"]
        );
        assert!(scan.suggestion.contains("status"), "{}", scan.suggestion);
        assert!(
            scan.suggestion.contains("listed existing indexes"),
            "{}",
            scan.suggestion
        );
        assert_eq!(scan.metrics.plan_rows, Some(Number::from(100)));
        assert_eq!(scan.metrics.actual_rows, Some(Number::from(2_000)));
        // A plan-only metric of a node that has none is omitted, not zeroed.
        assert_eq!(scan.metrics.startup_cost, None);
    }

    #[test]
    fn treats_a_small_sequential_scan_as_a_hint_and_handles_plan_only_estimates() {
        let payload = envelope(
            json!({
                "Node Type": "Seq Scan", "Relation Name": "tiny",
                "Plan Rows": 4, "Total Cost": 1.2
            }),
            json!({}),
        );
        let report = PlanAnalyzer::default().analyze(&payload, false).unwrap();

        assert!(report.clean());
        assert!(!report.warnings());
        let finding = &report.findings[0];
        assert_eq!(finding.rule, PlanRule::SequentialScan);
        assert_eq!(finding.severity, Severity::Hint);
        assert_eq!(finding.relation.as_deref(), Some("tiny"));
        assert!(finding.indexes.is_empty());
        assert!(
            finding.suggestion.contains("No change is implied"),
            "{}",
            finding.suggestion
        );
        assert_eq!(report.execution_time_ms, None);
    }

    #[test]
    fn reports_a_large_unindexed_scan_without_inventing_a_column_recommendation() {
        let payload = envelope(
            json!({
                "Node Type": "Seq Scan", "Schema": "public", "Relation Name": "large_table",
                "Plan Rows": 5_000, "Total Cost": 1
            }),
            json!({}),
        );
        let report = PlanAnalyzer::default().analyze(&payload, false).unwrap();
        assert_eq!(
            report.findings[0].suggestion,
            "Review selective predicates. No existing index was found for this relation."
        );
    }

    #[test]
    fn detects_external_sorts_and_ignores_low_volume_or_incomplete_runtime_metrics() {
        let payload = envelope(
            json!({
                "Node Type": "Append",
                "Plans": [
                    {"Node Type": "Sort", "Sort Method": "external merge", "Plan Rows": 2},
                    {"Node Type": "Nested Loop", "Plan Rows": 2},
                    {"Node Type": "Index Scan", "Plan Rows": 0, "Actual Rows": 500},
                    {"Node Type": "Index Scan", "Plan Rows": 100, "Actual Rows": 500}
                ]
            }),
            json!({}),
        );
        let report = PlanAnalyzer::default().analyze(&payload, false).unwrap();
        assert_eq!(
            report
                .findings
                .iter()
                .map(|finding| finding.rule)
                .collect::<Vec<_>>(),
            [PlanRule::DiskSort]
        );
    }

    #[test]
    fn rejects_malformed_postgresql_explain_payloads() {
        let analyzer = PlanAnalyzer::default();
        for payload in [json!([]), json!({"Plan": {}}), json!("EXPLAIN")] {
            assert_eq!(
                analyzer.analyze(&payload, false),
                Err(PlanError::InvalidPayload),
                "{payload}"
            );
        }
        assert_eq!(
            analyzer.analyze(&json!([{"Planning Time": 1}]), false),
            Err(PlanError::MissingPlan)
        );
    }

    /// The `--json` payload is the contract `mxrs db explain --json` prints.
    /// `clean` is computed, so it cannot drift from the findings beside it.
    #[test]
    fn the_json_payload_carries_the_derived_clean_flag_and_omits_absent_metrics() {
        let payload = envelope(
            json!({"Node Type": "Seq Scan", "Relation Name": "tiny", "Plan Rows": 4}),
            json!({}),
        );
        let report = PlanAnalyzer::default().analyze(&payload, false).unwrap();
        let rendered = serde_json::to_value(&report).unwrap();
        assert_eq!(rendered["engine"], json!("postgresql"));
        assert_eq!(rendered["clean"], json!(true));
        assert_eq!(rendered["analyzed"], json!(false));
        assert_eq!(rendered["total_cost"], Value::Null);
        assert_eq!(rendered["findings"][0]["rule"], json!("sequential_scan"));
        assert_eq!(rendered["findings"][0]["severity"], json!("hint"));
        assert_eq!(
            rendered["findings"][0]["metrics"],
            json!({"plan_rows": 4}),
            "absent metrics must not be serialized as null"
        );
        assert!(
            rendered.get("raw").is_none(),
            "the raw plan stays out of the payload"
        );
    }

    /// A relation named without a schema matches the catalog across schemas —
    /// the planner did not say which one, so neither does the report.
    #[test]
    fn a_scan_without_a_schema_matches_every_schema_in_the_catalog() {
        let payload = envelope(
            json!({
                "Node Type": "Seq Scan", "Relation Name": "sales$order",
                "Plan Rows": 5_000, "Filter": "(status = 'Open')"
            }),
            json!({}),
        );
        let report = PlanAnalyzer::new(catalog())
            .analyze(&payload, false)
            .unwrap();
        let finding = &report.findings[0];
        assert_eq!(
            finding
                .indexes
                .iter()
                .map(|index| index.name.as_str())
                .collect::<Vec<_>>(),
            ["sales_order_status_idx", "archive_idx"]
        );
        assert_eq!(
            finding.suggestion,
            "Review the filter \"(status = 'Open')\". \
             Compare predicates with the listed existing indexes."
        );
    }
}
