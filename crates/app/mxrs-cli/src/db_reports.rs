//! Human-readable rendering of the database command's analysis reports.
//!
//! These live in the library rather than beside the dispatcher so the text a
//! user actually reads is pinned by tests. Each renderer returns a `String`
//! instead of printing: the shape is the contract, and a function that writes
//! to stdout cannot be asserted on.

use mxrs_oql::plan::PlanReport;

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
