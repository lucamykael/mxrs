//! Declarative functional-test planning for existing Mendix projects.
//!
//! This is the executable, read-only half of `mxrb test --plan`: suites are
//! JSON rather than Ruby, and every target, hook, argument set, and count
//! entity is checked against the MPR before a plan is accepted. Executing the
//! model flow graph remains a separate runtime capability and is never
//! inferred from a successful plan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mxrs_ir::flow::{Activity, FlowReturnType, LogLevel, MicroflowCallMapping, MicroflowDecl};
use mxrs_model::{Module, Project};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum FunctionalError {
    #[error("cannot read functional test suite {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid functional test suite {path}: {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Model(#[from] mxrs_model::ModelError),
    #[error(transparent)]
    Writer(#[from] mxrs_writer::WriterError),
    #[error("functional test suite is empty")]
    Empty,
    #[error("functional test {test:?}: {message}")]
    Invalid { test: String, message: String },
}

pub type Result<T> = std::result::Result<T, FunctionalError>;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub tests: Vec<TestCase>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    pub name: String,
    pub call: String,
    #[serde(default, rename = "pass")]
    pub arguments: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout: f64,
    #[serde(default)]
    pub expect: Expectations,
    pub before: Option<Hook>,
    pub after: Option<Hook>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Expectations {
    #[serde(rename = "return")]
    pub return_value: Option<String>,
    #[serde(default)]
    pub count: Vec<CountExpectation>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CountExpectation {
    pub entity: String,
    pub xpath: Option<String>,
    pub equals: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub call: String,
    #[serde(default, rename = "pass")]
    pub arguments: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlannedTest {
    pub name: String,
    pub target: String,
    pub arguments: usize,
    pub count_assertions: usize,
    pub has_return_assertion: bool,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FunctionalPlan {
    pub project: PathBuf,
    pub definition: PathBuf,
    pub mendix_version: Option<String>,
    pub tests: Vec<PlannedTest>,
    pub execution_supported: bool,
}

struct Prepared {
    project: PathBuf,
    definition_path: PathBuf,
    mendix_version: Option<String>,
    definition: Definition,
}

pub fn plan(
    project_path: impl AsRef<Path>,
    definition_path: impl AsRef<Path>,
) -> Result<FunctionalPlan> {
    let prepared = prepare(project_path.as_ref(), definition_path.as_ref())?;
    let tests = prepared
        .definition
        .tests
        .iter()
        .map(|test| PlannedTest {
            name: test.name.trim().to_string(),
            target: test.call.clone(),
            arguments: test.arguments.len(),
            count_assertions: test.expect.count.len(),
            has_return_assertion: test.expect.return_value.is_some(),
            before: test.before.as_ref().map(|hook| hook.call.clone()),
            after: test.after.as_ref().map(|hook| hook.call.clone()),
        })
        .collect();
    Ok(FunctionalPlan {
        project: prepared.project,
        definition: prepared.definition_path,
        mendix_version: prepared.mendix_version,
        tests,
        execution_supported: false,
    })
}

pub fn instrument(
    project_path: impl AsRef<Path>,
    definition_path: impl AsRef<Path>,
) -> Result<mxrs_writer::InstrumentationReport> {
    const MODULE: &str = "MxrsTests";
    const RUNNER: &str = "MxrsTests.RunAll";
    let prepared = prepare(project_path.as_ref(), definition_path.as_ref())?;
    let mut flows = prepared
        .definition
        .tests
        .iter()
        .enumerate()
        .map(|(index, test)| wrapper(index, test))
        .collect::<Vec<_>>();
    flows.push(runner(&prepared.definition.tests));
    Ok(mxrs_writer::instrument_functional_tests(
        prepared.project,
        MODULE,
        RUNNER,
        &flows,
        prepared.definition.tests.len(),
    )?)
}

fn prepare(project_path: &Path, definition_path: &Path) -> Result<Prepared> {
    let project_path = absolute(project_path)?;
    let definition_path = absolute(definition_path)?;
    let source =
        std::fs::read_to_string(&definition_path).map_err(|source| FunctionalError::Io {
            path: definition_path.display().to_string(),
            source,
        })?;
    let definition: Definition =
        serde_json::from_str(&source).map_err(|source| FunctionalError::Json {
            path: definition_path.display().to_string(),
            source,
        })?;
    if definition.tests.is_empty() {
        return Err(FunctionalError::Empty);
    }

    let project = Project::open(&project_path, true)?;
    let modules = project.modules()?;
    let mut names = BTreeSet::new();
    for test in &definition.tests {
        validate_test(test, &modules, &mut names)?;
    }
    Ok(Prepared {
        project: project_path,
        definition_path,
        mendix_version: project.mendix_version()?,
        definition,
    })
}

fn wrapper(index: usize, test: &TestCase) -> MicroflowDecl {
    let mut flow = MicroflowDecl::new(format!("Test_{:03}", index + 1));
    flow.return_type = Some(FlowReturnType::Boolean);
    if let Some(hook) = &test.before {
        flow.activities.push(call(hook, None));
    }
    flow.activities.push(call(
        &Hook {
            call: test.call.clone(),
            arguments: test.arguments.clone(),
        },
        test.expect.return_value.as_ref().map(|_| "mxrs_actual"),
    ));
    let mut conditions = Vec::new();
    if let Some(expected) = &test.expect.return_value {
        conditions.push(format!("$mxrs_actual = {expected}"));
    }
    for (count_index, expectation) in test.expect.count.iter().enumerate() {
        let list = format!("mxrs_items_{}", count_index + 1);
        let count = format!("mxrs_count_{}", count_index + 1);
        flow.activities.push(Activity::RetrieveObjects {
            entity: expectation.entity.clone(),
            variable: list.clone(),
            xpath: expectation.xpath.clone(),
        });
        flow.activities.push(Activity::AggregateCount {
            list_variable: list,
            output_variable: count.clone(),
        });
        conditions.push(format!("${count} = {}", expectation.equals));
    }
    if let Some(hook) = &test.after {
        flow.activities.push(call(hook, None));
        flow.rescue_activities.push(call(hook, None));
    }
    flow.rescue_activities.push(Activity::ReturnValue {
        expression: "false".to_string(),
    });
    flow.return_expression = Some(if conditions.is_empty() {
        "true".to_string()
    } else {
        format!("({})", conditions.join(" and "))
    });
    flow
}

fn call(hook: &Hook, result: Option<&str>) -> Activity {
    Activity::CallMicroflow {
        name: hook.call.clone(),
        result_variable: result.map(str::to_string),
        use_return: result.is_some(),
        mappings: hook
            .arguments
            .iter()
            .map(|(parameter, value)| MicroflowCallMapping {
                parameter: parameter.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn runner(tests: &[TestCase]) -> MicroflowDecl {
    let mut flow = MicroflowDecl::new("RunAll");
    flow.return_type = Some(FlowReturnType::Boolean);
    for (index, test) in tests.iter().enumerate() {
        let variable = format!("passed_{}", index + 1);
        flow.activities.push(Activity::CallMicroflow {
            name: format!("MxrsTests.Test_{:03}", index + 1),
            result_variable: Some(variable.clone()),
            use_return: true,
            mappings: vec![],
        });
        flow.activities.push(Activity::Decision {
            condition: format!("${variable}"),
            true_branch: vec![log(
                format!("[MXRS_TEST] PASS {}", test.name),
                LogLevel::Info,
            )],
            false_branch: vec![log(
                format!("[MXRS_TEST] FAIL {}", test.name),
                LogLevel::Error,
            )],
        });
    }
    flow.activities
        .push(log("[MXRS_TEST] DONE".to_string(), LogLevel::Info));
    flow.return_expression = Some("true".to_string());
    flow
}

fn log(message: String, level: LogLevel) -> Activity {
    Activity::LogMessage {
        message,
        level,
        node: "'MXRS_TEST'".to_string(),
    }
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(|source| FunctionalError::Io {
        path: path.display().to_string(),
        source,
    })
}

fn validate_test(test: &TestCase, modules: &[Module], names: &mut BTreeSet<String>) -> Result<()> {
    let name = test.name.trim();
    invalid(test, name.is_empty(), "name cannot be empty")?;
    invalid(test, !names.insert(name.to_string()), "name is duplicated")?;
    invalid(
        test,
        !test.timeout.is_finite() || test.timeout <= 0.0,
        "timeout must be a positive finite number",
    )?;
    validate_call(test, &test.call, &test.arguments, modules, "microflow")?;
    if let Some(hook) = &test.before {
        validate_call(test, &hook.call, &hook.arguments, modules, "before hook")?;
    }
    if let Some(hook) = &test.after {
        validate_call(test, &hook.call, &hook.arguments, modules, "after hook")?;
    }
    for expectation in &test.expect.count {
        invalid(
            test,
            split_qualified(&expectation.entity).is_none(),
            &format!(
                "count entity {} is not a qualified Mendix name",
                expectation.entity
            ),
        )?;
        let found = modules.iter().any(|module| {
            let Some((module_name, entity_name)) = split_qualified(&expectation.entity) else {
                return false;
            };
            module.name.as_deref() == Some(module_name)
                && module
                    .entities()
                    .iter()
                    .any(|entity| entity.name.as_deref() == Some(entity_name))
        });
        invalid(
            test,
            !found,
            &format!("count entity {} was not found", expectation.entity),
        )?;
    }
    Ok(())
}

fn validate_call(
    test: &TestCase,
    target: &str,
    arguments: &BTreeMap<String, String>,
    modules: &[Module],
    label: &str,
) -> Result<()> {
    let Some((module_name, flow_name)) = split_qualified(target) else {
        return invalid(
            test,
            true,
            &format!("{label} call must be a qualified Mendix name"),
        );
    };
    let flow = modules
        .iter()
        .find(|module| module.name.as_deref() == Some(module_name))
        .and_then(|module| {
            module
                .microflows
                .iter()
                .find(|flow| flow.name.as_deref() == Some(flow_name))
        });
    let Some(flow) = flow else {
        return invalid(test, true, &format!("{label} {target} was not found"));
    };
    let parameters = flow
        .parameters
        .iter()
        .filter_map(|parameter| parameter.get_str("Name").ok().map(str::to_string))
        .collect::<BTreeSet<_>>();
    let supplied = arguments.keys().cloned().collect::<BTreeSet<_>>();
    let missing = parameters
        .difference(&supplied)
        .cloned()
        .collect::<Vec<_>>();
    let unknown = supplied
        .difference(&parameters)
        .cloned()
        .collect::<Vec<_>>();
    invalid(
        test,
        !missing.is_empty() || !unknown.is_empty(),
        &format!(
            "{label} {target} arguments mismatch (missing: {}, unknown: {})",
            missing.join(", "),
            unknown.join(", ")
        ),
    )
}

fn invalid(test: &TestCase, condition: bool, message: &str) -> Result<()> {
    if condition {
        Err(FunctionalError::Invalid {
            test: test.name.clone(),
            message: message.to_string(),
        })
    } else {
        Ok(())
    }
}

fn split_qualified(value: &str) -> Option<(&str, &str)> {
    let (module, name) = value.split_once('.')?;
    if valid_identifier(module) && valid_identifier(name) && !name.contains('.') {
        Some((module, name))
    } else {
        None
    }
}

fn valid_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn default_timeout() -> f64 {
    60.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("Orders.mpr");
        let mut builder = mxrs_dsl::ProjectBuilder::new("11.12.1");
        builder.module("Sales", |module| {
            module.entity("Order", |_| {});
            module.microflow("Setup", |_| {});
            module.microflow("Create", |_| {});
            module.microflow("Cleanup", |_| {});
        });
        mxrs_writer::write_project(&project, &builder.build()).unwrap();
        (directory, project)
    }

    #[test]
    fn plans_a_suite_only_after_validating_targets_arguments_hooks_and_entities() {
        let (directory, project) = fixture();
        let definition = directory.path().join("functional.json");
        std::fs::write(
            &definition,
            r#"{
  "tests": [{
    "name": "creates an order",
    "call": "Sales.Create",
    "expect": {
      "return": "true",
      "count": [{ "entity": "Sales.Order", "xpath": null, "equals": 1 }]
    },
    "before": { "call": "Sales.Setup" },
    "after": { "call": "Sales.Cleanup" }
  }]
}"#,
        )
        .unwrap();

        let plan = plan(&project, &definition).unwrap();
        assert_eq!(plan.mendix_version.as_deref(), Some("11.12.1"));
        assert_eq!(plan.tests[0].arguments, 0);
        assert_eq!(plan.tests[0].count_assertions, 1);
        assert!(plan.tests[0].has_return_assertion);
        assert!(!plan.execution_supported);
    }

    #[test]
    fn rejects_empty_duplicate_malformed_and_unresolvable_suites() {
        let (directory, project) = fixture();
        for (name, body, expected) in [
            ("empty", r#"{"tests":[]}"#, "suite is empty"),
            (
                "missing",
                r#"{"tests":[{"name":"x","call":"Sales.Missing"}]}"#,
                "was not found",
            ),
            (
                "arguments",
                r#"{"tests":[{"name":"x","call":"Sales.Create","pass":{"Other":"1"}}]}"#,
                "missing: , unknown: Other",
            ),
            (
                "entity",
                r#"{"tests":[{"name":"x","call":"Sales.Setup","expect":{"count":[{"entity":"Sales.Missing","equals":0}]}}]}"#,
                "count entity Sales.Missing was not found",
            ),
        ] {
            let definition = directory.path().join(format!("{name}.json"));
            std::fs::write(&definition, body).unwrap();
            assert!(
                plan(&project, definition)
                    .unwrap_err()
                    .to_string()
                    .contains(expected)
            );
        }
    }

    #[test]
    fn wrapper_carries_return_count_hooks_and_a_false_rescue_path() {
        let definition: Definition = serde_json::from_str(
            r#"{"tests":[{"name":"x","call":"Sales.Create","expect":{"return":"true","count":[{"entity":"Sales.Order","equals":1}]},"before":{"call":"Sales.Setup"},"after":{"call":"Sales.Cleanup"}}]}"#,
        )
        .unwrap();
        let flow = wrapper(0, &definition.tests[0]);
        assert_eq!(
            flow.return_expression.as_deref(),
            Some("($mxrs_actual = true and $mxrs_count_1 = 1)")
        );
        assert!(matches!(
            &flow.activities[1],
            Activity::CallMicroflow {
                result_variable: Some(variable),
                use_return: true,
                ..
            } if variable == "mxrs_actual"
        ));
        assert!(matches!(
            flow.rescue_activities.last(),
            Some(Activity::ReturnValue { expression }) if expression == "false"
        ));
    }
}
