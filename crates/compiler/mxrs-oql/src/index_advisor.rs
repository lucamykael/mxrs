//! Evidence-backed index hypotheses.
//!
//! Ports `Mxrb::Oql::IndexAdvisor`. It proposes an index only where two
//! independent signals meet: a relation under cumulative sequential-scan
//! pressure, and a column the recorded workload repeatedly filters on. Neither
//! alone is enough, and nothing here reads a query plan — this is a hypothesis
//! to test with `db explain`, not a recommendation to apply.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::workload::{WorkloadQuery, WorkloadReport, WorkloadRule};

/// The relation after a `FROM`/`JOIN`. Deliberately shallow: it reads the
/// recorded statement text, which is normalized SQL, not a parse tree.
static TABLE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(?:FROM|JOIN)\s+["\[]?([0-9A-Za-z_$]+)["\]]?"#)
        .expect("the table pattern is a valid regex")
});

/// A column on the left of a comparison. Also shallow, and the reason a
/// candidate is a hypothesis rather than a conclusion.
static PREDICATE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([A-Za-z_][0-9A-Za-z_]*)\s*(?:=|<|>|<=|>=|LIKE|IN\s*\()")
        .expect("the predicate pattern is a valid regex")
});

/// The first non-empty parenthesized group of an index definition.
static COLUMNS_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([^)]+)\)").expect("the columns pattern is a valid regex"));

const REASON: &str =
    "Repeated filtered workload coincides with cumulative sequential-scan pressure.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
}

impl Confidence {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
        }
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexCandidate {
    pub relation: String,
    pub columns: Vec<String>,
    pub confidence: Confidence,
    pub query_ids: Vec<String>,
    pub reason: &'static str,
}

/// Two indexes on one relation whose column lists are identical after
/// normalization. Named as a pair rather than picking a winner: which one to
/// drop depends on constraints and naming the advisor cannot see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RedundantPair(pub String, pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexAdvice {
    pub candidates: Vec<IndexCandidate>,
    pub redundant_indexes: Vec<RedundantPair>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IndexAdvisor;

impl IndexAdvisor {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    #[must_use]
    pub fn analyze(&self, report: &WorkloadReport) -> IndexAdvice {
        let pressured = pressured_relations(report);
        IndexAdvice {
            candidates: candidates_for(&collect_evidence(&report.queries), &pressured),
            redundant_indexes: redundant(&report.index_stats),
        }
    }
}

/// `(relation, column)` pairs to the queries that supply the evidence, in the
/// order the workload produced them — the report lists candidates in the order
/// the evidence appeared, not in an order derived from a hash.
type Evidence<'a> = Vec<((String, String), Vec<&'a WorkloadQuery>)>;

fn collect_evidence(queries: &[WorkloadQuery]) -> Evidence<'_> {
    let mut evidence: Evidence<'_> = Vec::new();
    for query in queries {
        for relation in captures(&TABLE_PATTERN, &query.query) {
            for column in captures(&PREDICATE_PATTERN, &query.query) {
                let key = (relation.to_lowercase(), column.to_lowercase());
                match evidence.iter_mut().find(|(existing, _)| *existing == key) {
                    Some((_, supporting)) => supporting.push(query),
                    None => evidence.push((key, vec![query])),
                }
            }
        }
    }
    evidence
}

/// The first capture of every match, de-duplicated, first-seen order.
fn captures(pattern: &Regex, text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for capture in pattern.captures_iter(text) {
        let value = capture[1].to_string();
        if !found.contains(&value) {
            found.push(value);
        }
    }
    found
}

fn pressured_relations(report: &WorkloadReport) -> Vec<String> {
    report
        .findings
        .iter()
        .filter(|finding| finding.rule == WorkloadRule::TableSequentialPressure)
        .filter_map(|finding| finding.subject.split('.').next_back())
        .map(str::to_lowercase)
        .collect()
}

fn candidates_for(evidence: &Evidence<'_>, pressured: &[String]) -> Vec<IndexCandidate> {
    evidence
        .iter()
        .filter(|((relation, _), _)| pressured.contains(relation))
        .filter(|(_, queries)| sufficient_evidence(queries))
        .map(|((relation, column), queries)| IndexCandidate {
            relation: relation.clone(),
            columns: vec![column.clone()],
            confidence: confidence(queries),
            query_ids: distinct_ids(queries),
            reason: REASON,
        })
        .collect()
}

/// Two distinct fingerprints, or one expensive enough to matter on its own.
fn sufficient_evidence(queries: &[&WorkloadQuery]) -> bool {
    distinct_ids(queries).len() >= 2 || total_time(queries) >= 1_000.0
}

fn confidence(queries: &[&WorkloadQuery]) -> Confidence {
    if distinct_ids(queries).len() >= 3 || total_time(queries) >= 5_000.0 {
        Confidence::High
    } else {
        Confidence::Medium
    }
}

fn distinct_ids(queries: &[&WorkloadQuery]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for query in queries {
        if !ids.contains(&query.query_id) {
            ids.push(query.query_id.clone());
        }
    }
    ids
}

/// Ruby's `Array#sum` compensates for floating-point error
/// (Kahan-Babuška-Neumaier) rather than folding naively, and this sum is
/// compared against a threshold: on a long workload the two can land on
/// opposite sides of 1000 ms for the same inputs.
fn total_time(queries: &[&WorkloadQuery]) -> f64 {
    let (mut sum, mut compensation) = (0.0_f64, 0.0_f64);
    for query in queries {
        let value = query.total_time_ms;
        let next = sum + value;
        compensation += if sum.abs() >= value.abs() {
            (sum - next) + value
        } else {
            (value - next) + sum
        };
        sum = next;
    }
    sum + compensation
}

/// A `(schema, relation)` pair, as the catalog spells it.
type Relation = (String, String);

/// An index's `(name, normalized columns)`, the form two indexes are compared
/// in to decide whether they overlap.
type Signature = (String, String);

fn redundant(indexes: &[Map<String, Value>]) -> Vec<RedundantPair> {
    let mut groups: Vec<(Relation, Vec<Signature>)> = Vec::new();
    for index in indexes {
        let Some((relation, signature)) = index_signature(index) else {
            continue;
        };
        match groups
            .iter_mut()
            .find(|(existing, _)| *existing == relation)
        {
            Some((_, entries)) => entries.push(signature),
            None => groups.push((relation, vec![signature])),
        }
    }
    groups
        .iter()
        .flat_map(|(_, entries)| {
            entries.iter().enumerate().flat_map(move |(at, left)| {
                entries[at + 1..]
                    .iter()
                    .filter(move |right| left.1 == right.1)
                    .map(move |right| RedundantPair(left.0.clone(), right.0.clone()))
            })
        })
        .collect()
}

/// `(schema, relation)` and `(index name, normalized columns)`.
///
/// The column list is the first parenthesized group of the definition, which
/// is what mxrb reads. On an expression index (`btree (lower(name))`) that
/// stops at the inner `)` and yields `lower(name` — a stable, harmless
/// signature, since it is only ever compared against another signature read
/// the same way.
fn index_signature(index: &Map<String, Value>) -> Option<(Relation, Signature)> {
    let definition = index.get("indexdef").and_then(Value::as_str)?;
    let columns = COLUMNS_PATTERN.captures(definition)?;
    Some((
        (text(index, "schemaname"), text(index, "relname")),
        (text(index, "indexrelname"), normalize(&columns[1])),
    ))
}

fn text(row: &Map<String, Value>, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn normalize(columns: &str) -> String {
    columns
        .to_lowercase()
        .chars()
        .filter(|character| !matches!(character, '"' | '[' | ']') && !character.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::plan::{PlanEngine, Severity};
    use crate::workload::{WorkloadFinding, WorkloadMetrics};

    fn query(id: &str, sql: &str, total: f64) -> WorkloadQuery {
        WorkloadQuery {
            query_id: id.to_string(),
            query: sql.to_string(),
            calls: 2,
            total_time_ms: total,
            mean_time_ms: 200.0,
            rows: 100,
            shared_hits: 10,
            shared_reads: 2,
            temp_writes: 0,
            io_time_ms: 1.0,
            cache_hit_ratio: 0.8,
        }
    }

    fn pressure(subject: &str) -> WorkloadFinding {
        WorkloadFinding {
            rule: WorkloadRule::TableSequentialPressure,
            severity: Severity::Warning,
            subject: subject.to_string(),
            message: String::new(),
            suggestion: String::new(),
            metrics: WorkloadMetrics::Table {
                seq_scan: 0,
                seq_tup_read: 0,
                idx_scan: 0,
                live_rows: 0,
            },
        }
    }

    fn report(
        queries: Vec<WorkloadQuery>,
        findings: Vec<WorkloadFinding>,
        indexes: Value,
    ) -> WorkloadReport {
        WorkloadReport {
            engine: PlanEngine::PostgreSql,
            queries,
            table_stats: Vec::new(),
            index_stats: indexes
                .as_array()
                .expect("an array of rows")
                .iter()
                .map(|row| row.as_object().expect("a row object").clone())
                .collect(),
            findings,
        }
    }

    /// Mirrors mxrb's `performance_advisor_spec.rb` index-advice case.
    #[test]
    fn suggests_only_evidence_backed_columns_and_detects_duplicate_definitions() {
        let advice = IndexAdvisor::new().analyze(&report(
            vec![
                query("q1", "SELECT * FROM sales$order WHERE status = 1", 600.0),
                query("q2", "SELECT id FROM sales$order WHERE status = 2", 600.0),
            ],
            vec![pressure("public.sales$order")],
            json!([
                {"schemaname": "public", "relname": "sales$order", "indexrelname": "a",
                 "indexdef": "CREATE INDEX a ON sales$order (status)"},
                {"schemaname": "public", "relname": "sales$order", "indexrelname": "b",
                 "indexdef": "CREATE INDEX b ON sales$order (\"status\")"}
            ]),
        ));

        let candidate = &advice.candidates[0];
        assert_eq!(candidate.relation, "sales$order");
        assert_eq!(candidate.columns, ["status"]);
        assert_eq!(candidate.confidence, Confidence::Medium);
        assert_eq!(candidate.query_ids, ["q1", "q2"]);
        assert_eq!(
            advice.redundant_indexes,
            [RedundantPair("a".to_string(), "b".to_string())]
        );
    }

    /// Both signals are required. Pressure with no repeated predicate, and a
    /// repeated predicate with no pressure, each yield nothing.
    #[test]
    fn a_candidate_needs_both_sequential_pressure_and_repeated_evidence() {
        let queries = vec![query("q1", "SELECT * FROM orders WHERE status = 1", 10.0)];
        // Repeated filtering, but the relation is not under pressure.
        assert!(
            IndexAdvisor::new()
                .analyze(&report(queries.clone(), vec![], json!([])))
                .candidates
                .is_empty()
        );
        // Pressure, but a single cheap fingerprint is not enough evidence.
        assert!(
            IndexAdvisor::new()
                .analyze(&report(queries, vec![pressure("public.orders")], json!([])))
                .candidates
                .is_empty()
        );
        // One expensive fingerprint on its own clears the bar.
        let advice = IndexAdvisor::new().analyze(&report(
            vec![query(
                "q1",
                "SELECT * FROM orders WHERE status = 1",
                1_000.0,
            )],
            vec![pressure("public.orders")],
            json!([]),
        ));
        assert_eq!(advice.candidates[0].confidence, Confidence::Medium);
        // Three fingerprints, or one very expensive one, raise the confidence.
        let advice = IndexAdvisor::new().analyze(&report(
            vec![query(
                "q1",
                "SELECT * FROM orders WHERE status = 1",
                6_000.0,
            )],
            vec![pressure("public.orders")],
            json!([]),
        ));
        assert_eq!(advice.candidates[0].confidence, Confidence::High);
    }

    /// Redundancy is per relation, and only across identical column lists.
    #[test]
    fn overlapping_indexes_are_paired_per_relation_and_never_across_relations() {
        let advice = IndexAdvisor::new().analyze(&report(
            vec![],
            vec![],
            json!([
                {"schemaname": "public", "relname": "orders", "indexrelname": "a",
                 "indexdef": "CREATE INDEX a ON orders USING btree (status, id)"},
                {"schemaname": "public", "relname": "orders", "indexrelname": "b",
                 "indexdef": "CREATE INDEX b ON orders USING btree ( STATUS , ID )"},
                {"schemaname": "public", "relname": "orders", "indexrelname": "c",
                 "indexdef": "CREATE INDEX c ON orders USING btree (id)"},
                {"schemaname": "public", "relname": "invoices", "indexrelname": "d",
                 "indexdef": "CREATE INDEX d ON invoices USING btree (id)"},
                {"schemaname": "public", "relname": "invoices", "indexrelname": "e"}
            ]),
        ));
        assert_eq!(
            advice.redundant_indexes,
            [RedundantPair("a".to_string(), "b".to_string())]
        );
    }

    #[test]
    fn the_json_payload_names_candidates_and_pairs() {
        let advice = IndexAdvisor::new().analyze(&report(
            vec![query(
                "q1",
                "SELECT * FROM orders WHERE status = 1",
                6_000.0,
            )],
            vec![pressure("public.orders")],
            json!([]),
        ));
        let rendered = serde_json::to_value(&advice).unwrap();
        assert_eq!(
            rendered["candidates"][0],
            json!({
                "relation": "orders",
                "columns": ["status"],
                "confidence": "high",
                "query_ids": ["q1"],
                "reason": REASON,
            })
        );
        assert_eq!(rendered["redundant_indexes"], json!([]));
    }

    /// A redundant pair serializes as a two-element array, the way mxrb's
    /// `[left_name, right_name]` does — not as an object with field names
    /// nobody on the other side would recognize.
    #[test]
    fn a_redundant_pair_serializes_as_the_two_names_in_order() {
        let advice = IndexAdvisor::new().analyze(&report(
            vec![],
            vec![],
            json!([
                {"schemaname": "public", "relname": "orders", "indexrelname": "a",
                 "indexdef": "CREATE INDEX a ON orders USING btree (status)"},
                {"schemaname": "public", "relname": "orders", "indexrelname": "b",
                 "indexdef": "CREATE INDEX b ON orders USING btree (\"status\")"},
                {"schemaname": "public", "relname": "orders", "indexrelname": "c",
                 "indexdef": "CREATE INDEX c ON orders USING btree (status)"}
            ]),
        ));
        assert_eq!(
            serde_json::to_value(&advice).unwrap()["redundant_indexes"],
            json!([["a", "b"], ["a", "c"], ["b", "c"]]),
            "every overlapping pair is named, in catalog order"
        );
    }
}
