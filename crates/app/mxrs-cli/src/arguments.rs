pub fn validate_options(
    arguments: &[String],
    values: &[&str],
    flags: &[&str],
) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        let is_value = values.contains(&argument);
        if is_value || flags.contains(&argument) {
            if !seen.insert(argument) {
                return Err(format!("duplicate option {argument}"));
            }
            if is_value {
                if arguments
                    .get(index + 1)
                    .is_none_or(|value| value.is_empty() || value.starts_with('-'))
                {
                    return Err(format!("{argument} requires a value"));
                }
                index += 1;
            }
        } else if argument.starts_with('-') {
            return Err(format!(
                "unknown option {argument:?}; prefix a dash-leading filename with ./"
            ));
        }
        index += 1;
    }
    Ok(())
}

/// Missing values stay in the argument list so each command's strict arity
/// check rejects them; an option cannot accidentally consume another option.
pub fn take_value(arguments: &mut Vec<String>, flag: &str) -> Option<String> {
    let position = arguments.iter().position(|argument| argument == flag)?;
    let value = arguments.get(position + 1)?;
    if value.starts_with('-') {
        return None;
    }
    arguments.remove(position);
    Some(arguments.remove(position))
}

pub fn take_flag(arguments: &mut Vec<String>, flag: &str) -> bool {
    if let Some(position) = arguments.iter().position(|argument| argument == flag) {
        arguments.remove(position);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_options_cannot_be_reinterpreted_as_positional_names() {
        for raw in [
            vec!["--limit", "--json"],
            vec!["--limit"],
            vec!["--limit", ""],
            vec!["--unknown"],
            vec!["--json", "--json"],
            vec!["--limit", "1", "--limit", "2"],
        ] {
            let args = raw.iter().map(ToString::to_string).collect::<Vec<_>>();
            assert!(
                validate_options(&args, &["--limit"], &["--json"]).is_err(),
                "{raw:?}"
            );
        }
        assert!(
            validate_options(
                &[
                    "project.mpr".into(),
                    "--limit".into(),
                    "2".into(),
                    "word".into(),
                    "--json".into()
                ],
                &["--limit"],
                &["--json"]
            )
            .is_ok()
        );
        assert!(validate_options(&[], &[], &[]).is_ok());
    }

    #[test]
    fn option_values_are_removed_without_reordering_positionals() {
        let mut args = ["before", "--output", "path", "after"]
            .map(str::to_string)
            .to_vec();
        assert_eq!(take_value(&mut args, "--output").as_deref(), Some("path"));
        assert_eq!(args, ["before", "after"]);
        assert!(take_value(&mut args, "--output").is_none());
    }

    #[test]
    fn missing_values_and_duplicate_flags_remain_visible_to_arity_checks() {
        for raw in [vec!["--output"], vec!["--output", "--release"]] {
            let mut args = raw.iter().map(ToString::to_string).collect();
            assert!(take_value(&mut args, "--output").is_none());
            assert_eq!(args, raw);
        }
        let mut args = ["--json", "--json", "model.mpr"]
            .map(str::to_string)
            .to_vec();
        assert!(take_flag(&mut args, "--json"));
        assert_eq!(args, ["--json", "model.mpr"]);
        assert!(!take_flag(&mut args, "--other"));
        assert!(take_flag(&mut args, "--json"));
        assert_eq!(args, ["model.mpr"]);
    }
}
