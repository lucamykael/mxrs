//! Readiness uses covered/count equality, not a rounded percentage. Development
//! floors are a separate, fixed ratchet; passing those must never be presented
//! as release readiness or as evidence of Studio Pro compatibility.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

const METRICS: [(&str, u64); 4] = [
    ("lines", 82),
    ("functions", 82),
    ("regions", 81),
    ("branches", 66),
];

#[derive(Debug, Clone, Deserialize)]
struct Metric {
    count: u64,
    covered: u64,
    percent: f64,
    notcovered: Option<u64>,
}

impl Metric {
    fn validate(&self, context: &str) -> Result<(), String> {
        if self.covered > self.count {
            return Err(format!("{context}: covered exceeds count"));
        }
        if self
            .notcovered
            .is_some_and(|uncovered| uncovered != self.count - self.covered)
        {
            return Err(format!("{context}: inconsistent notcovered count"));
        }
        let expected = self.percent();
        if !self.percent.is_finite() || (self.percent - expected).abs() > 0.01 {
            return Err(format!("{context}: percentage disagrees with counts"));
        }
        Ok(())
    }

    fn percent(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.covered as f64 * 100.0 / self.count as f64
        }
    }
}

#[derive(Debug, Deserialize)]
struct Export {
    #[serde(rename = "type")]
    kind: String,
    version: String,
    data: Vec<Data>,
}

#[derive(Debug, Deserialize)]
struct Data {
    totals: BTreeMap<String, Metric>,
    files: Vec<File>,
}

#[derive(Debug, Deserialize)]
struct File {
    filename: String,
    summary: BTreeMap<String, Metric>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub scope: &'static str,
    pub metrics: Vec<MetricReport>,
    pub files: usize,
    pub development_passes: bool,
    pub coverage_complete: bool,
    pub uncovered_files: Vec<FileReport>,
}

#[derive(Debug, Serialize)]
pub struct MetricReport {
    pub metric: &'static str,
    pub count: u64,
    pub covered: u64,
    pub uncovered: u64,
    pub percent: f64,
    pub development_floor: u64,
    pub development_passes: bool,
}

#[derive(Debug, Serialize)]
pub struct FileReport {
    pub filename: String,
    pub uncovered: BTreeMap<String, u64>,
}

pub fn evaluate(bytes: &[u8]) -> Result<Report, String> {
    let export: Export = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid LLVM coverage JSON: {error}"))?;
    if export.kind != "llvm.coverage.json.export"
        || export.version.split('.').next() != Some("3")
        || export.data.len() != 1
    {
        return Err("expected one LLVM coverage export, format version 3.x".into());
    }
    let data = &export.data[0];
    if data.files.is_empty() {
        return Err("coverage export contains no files".into());
    }
    let mut names = BTreeSet::new();
    let mut uncovered_files = Vec::new();
    let mut sums: BTreeMap<&str, (u128, u128)> = BTreeMap::new();
    for file in &data.files {
        if file.filename.trim().is_empty() || !names.insert(&file.filename) {
            return Err(format!(
                "empty or duplicate coverage filename: {:?}",
                file.filename
            ));
        }
        let mut uncovered = BTreeMap::new();
        for (name, _) in METRICS {
            let metric = get_metric(&file.summary, name, &file.filename)?;
            metric.validate(&format!("{}: {name}", file.filename))?;
            let sum = sums.entry(name).or_default();
            sum.0 += u128::from(metric.count);
            sum.1 += u128::from(metric.covered);
            if metric.covered < metric.count {
                uncovered.insert(name.to_string(), metric.count - metric.covered);
            }
        }
        if !uncovered.is_empty() {
            uncovered_files.push(FileReport {
                filename: file.filename.clone(),
                uncovered,
            });
        }
    }
    let mut metrics = Vec::new();
    for (name, floor) in METRICS {
        let total = get_metric(&data.totals, name, "totals")?;
        total.validate(name)?;
        if total.count == 0 {
            return Err(format!(
                "{name}: zero measurable items; missing instrumentation cannot pass readiness"
            ));
        }
        if sums[name] != (u128::from(total.count), u128::from(total.covered)) {
            return Err(format!("{name}: totals disagree with per-file summaries"));
        }
        metrics.push(MetricReport {
            metric: name,
            count: total.count,
            covered: total.covered,
            uncovered: total.count - total.covered,
            percent: total.percent(),
            development_floor: floor,
            development_passes: u128::from(total.covered) * 100
                >= u128::from(total.count) * u128::from(floor),
        });
    }
    uncovered_files.sort_by(|left, right| left.filename.cmp(&right.filename));
    Ok(Report {
        scope: "all_instrumented_files_in_supplied_llvm_export_not_behavioral_parity",
        development_passes: metrics.iter().all(|metric| metric.development_passes),
        coverage_complete: metrics.iter().all(|metric| metric.uncovered == 0),
        files: data.files.len(),
        metrics,
        uncovered_files,
    })
}

fn get_metric<'a>(
    metrics: &'a BTreeMap<String, Metric>,
    name: &str,
    context: &str,
) -> Result<&'a Metric, String> {
    metrics
        .get(name)
        .ok_or_else(|| format!("{context}: missing {name} coverage; use --branch instrumentation"))
}

pub fn enforce(report: &Report, require_complete: bool) -> Result<(), String> {
    let failures = report
        .metrics
        .iter()
        .filter(|metric| {
            if require_complete {
                metric.uncovered != 0
            } else {
                !metric.development_passes
            }
        })
        .map(|metric| {
            format!(
                "{}: {}/{} covered ({:.4}%), {} uncovered; required {}%",
                metric.metric,
                metric.covered,
                metric.count,
                metric.percent,
                metric.uncovered,
                if require_complete {
                    100
                } else {
                    metric.development_floor
                },
            )
        })
        .collect::<Vec<_>>();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn export(count: u64, covered: u64) -> Value {
        let metrics = METRICS
            .iter()
            .map(|(name, _)| {
                (
                    *name,
                    json!({
                        "count": count,
                        "covered": covered,
                        "notcovered": count.saturating_sub(covered),
                        "percent": if count == 0 {0.0} else {covered as f64 * 100.0 / count as f64},
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        json!({
            "type": "llvm.coverage.json.export", "version": "3.1.0",
            "data": [{"totals": metrics, "files": [{"filename": "src/lib.rs", "summary": metrics}]}],
        })
    }

    fn check(value: &Value) -> Result<Report, String> {
        evaluate(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn complete_counts_pass_both_gates_and_do_not_create_uncovered_files() {
        let report = check(&export(10, 10)).unwrap();
        assert!(report.coverage_complete);
        assert!(report.development_passes);
        assert!(report.uncovered_files.is_empty());
        assert!(enforce(&report, false).is_ok());
        assert!(enforce(&report, true).is_ok());
    }

    #[test]
    fn development_floors_never_imply_release_readiness() {
        let report = check(&export(100, 82)).unwrap();
        assert!(report.development_passes);
        assert!(!report.coverage_complete);
        assert!(enforce(&report, false).is_ok());
        assert!(enforce(&report, true).unwrap_err().contains("18 uncovered"));
        assert_eq!(report.uncovered_files[0].uncovered["branches"], 18);
    }

    #[test]
    fn isolated_baseline_floors_use_exact_counts_for_each_metric() {
        for (name, floor) in [
            ("lines", 82),
            ("functions", 82),
            ("regions", 81),
            ("branches", 66),
        ] {
            for deficit in [0, 1] {
                let mut value = export(10_000, 10_000);
                let covered = floor * 100 - deficit;
                let metric = json!({
                    "count": 10_000,
                    "covered": covered,
                    "notcovered": 10_000 - covered,
                    "percent": covered as f64 / 100.0,
                });
                value["data"][0]["totals"][name] = metric.clone();
                value["data"][0]["files"][0]["summary"][name] = metric;
                let report = check(&value).unwrap();
                let actual = report
                    .metrics
                    .iter()
                    .find(|metric| metric.metric == name)
                    .unwrap();
                assert_eq!(actual.development_floor, floor);
                assert_eq!(report.development_passes, deficit == 0);
                assert_eq!(enforce(&report, false).is_ok(), deficit == 0);
                assert!(!report.coverage_complete);
                assert!(enforce(&report, true).is_err());
            }
        }
    }

    #[test]
    fn development_regressions_report_each_violated_metric() {
        let report = check(&export(100, 46)).unwrap();
        let failures = enforce(&report, false).unwrap_err();
        for (name, _) in METRICS {
            assert!(failures.contains(name));
        }
    }

    #[test]
    fn a_rounded_hundred_percent_cannot_hide_one_uncovered_branch() {
        let mut value = export(1_000_000, 999_999);
        value["data"][0]["totals"]["branches"]["percent"] = json!(100.0);
        value["data"][0]["files"][0]["summary"]["branches"]["percent"] = json!(100.0);
        let report = check(&value).unwrap();
        assert!(!report.coverage_complete);
        assert!(enforce(&report, true).unwrap_err().contains("branches"));
    }

    #[test]
    fn absent_instrumentation_is_not_vacuously_one_hundred_percent() {
        assert!(
            check(&export(0, 0))
                .unwrap_err()
                .contains("zero measurable")
        );
        let mut value = export(10, 10);
        value["data"][0]["totals"]
            .as_object_mut()
            .unwrap()
            .remove("branches");
        assert!(check(&value).unwrap_err().contains("missing branches"));
        let mut value = export(10, 10);
        value["data"][0]["files"][0]["summary"]
            .as_object_mut()
            .unwrap()
            .remove("branches");
        assert!(check(&value).unwrap_err().contains("missing branches"));
    }

    #[test]
    fn malformed_counts_percentages_and_aggregates_fail_closed() {
        for (field, replacement, expected) in [
            ("covered", json!(11), "covered exceeds count"),
            ("notcovered", json!(1), "inconsistent notcovered"),
            ("percent", json!(1.0), "percentage disagrees"),
        ] {
            for location in ["totals", "file"] {
                let mut value = export(10, 10);
                let metric = if location == "totals" {
                    &mut value["data"][0]["totals"]["lines"]
                } else {
                    &mut value["data"][0]["files"][0]["summary"]["lines"]
                };
                metric[field] = replacement.clone();
                assert!(check(&value).unwrap_err().contains(expected));
            }
        }
        let mut value = export(10, 10);
        value["data"][0]["totals"]["lines"]["count"] = json!(20);
        value["data"][0]["totals"]["lines"]["covered"] = json!(20);
        assert!(check(&value).unwrap_err().contains("per-file summaries"));
    }

    #[test]
    fn malformed_exports_and_duplicate_files_are_rejected() {
        assert!(evaluate(b"not json").is_err());
        for (field, replacement) in [
            ("type", json!("other")),
            ("version", json!("4.0.0")),
            ("data", json!([])),
        ] {
            let mut value = export(10, 10);
            value[field] = replacement;
            assert!(check(&value).is_err());
        }
        let mut value = export(10, 10);
        value["data"][0]["files"] = json!([]);
        assert!(check(&value).unwrap_err().contains("no files"));
        let mut value = export(10, 10);
        value["data"][0]["files"][0]["filename"] = json!(" ");
        assert!(check(&value).unwrap_err().contains("empty or duplicate"));
        let mut value = export(10, 10);
        let duplicate = value["data"][0]["files"][0].clone();
        value["data"][0]["files"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(check(&value).unwrap_err().contains("empty or duplicate"));
    }

    #[test]
    fn files_without_branches_are_valid_when_the_workspace_has_branch_instrumentation() {
        let mut value = export(10, 10);
        let mut empty_file = export(0, 0)["data"][0]["files"][0].clone();
        empty_file["filename"] = json!("src/constants.rs");
        value["data"][0]["files"]
            .as_array_mut()
            .unwrap()
            .push(empty_file);
        assert!(check(&value).unwrap().coverage_complete);
    }

    #[test]
    fn count_math_does_not_overflow_for_large_valid_llvm_counts() {
        let report = check(&export(u64::MAX, u64::MAX)).unwrap();
        assert!(report.coverage_complete);
        assert!(enforce(&report, false).is_ok());
    }
}
