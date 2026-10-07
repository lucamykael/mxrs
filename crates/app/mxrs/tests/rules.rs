//! A rule is a flow a decision asks to choose its way: declared as one, on
//! its own or in a service, and written with the fields a rule stores.

use mxrs::prelude::*;

/// Whether a text says nothing.
#[rule(module = "Text", name = "IsEmptyString")]
pub fn is_empty_string(flow: &mut FlowBuilder) {
    flow.parameter::<MxString>("Value", |_| {});
}

pub struct TextService;

#[service(module = "Text")]
impl TextService {
    #[rule(name = "IsLong")]
    pub fn is_long(flow: &mut FlowBuilder) {
        flow.parameter::<MxString>("Value", |_| {});
    }
}

#[mxrs::application(version = "11.12.1")]
pub struct Application;

#[test]
fn a_rule_is_declared_and_written_as_a_rule() {
    let project = Application::build();
    let text = project
        .modules
        .iter()
        .find(|module| module.name == "Text")
        .expect("the Text module is declared");
    let mut names: Vec<&str> = text.rules.iter().map(|rule| rule.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["IsEmptyString", "IsLong"]);
    assert!(text.microflows.is_empty());
    assert_eq!(
        <IsEmptyString as RuleMarker>::qualified_name(),
        "Text.IsEmptyString"
    );

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("App.mpr");
    mxrs::write_project(&path, &project).unwrap();
    let model = mxrs_model::Project::open(&path, true).unwrap();
    let rules: Vec<_> = model
        .all_units()
        .unwrap()
        .iter()
        .filter_map(|unit| model.mpr().parse_contents(unit).ok())
        .filter(|document| document.get_str("$Type").ok() == Some("Microflows$Rule"))
        .collect();
    assert_eq!(rules.len(), 2);
    for rule in rules {
        assert!(!rule.contains_key("AllowedModuleRoles"));
        assert!(!rule.contains_key("AllowConcurrentExecution"));
    }
}
