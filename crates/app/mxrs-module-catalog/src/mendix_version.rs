//! A manifest's `mendix_version` constraint string against the project's
//! actual Mendix version. Ports
//! `Mxrb::Marketplace::Installer#mendix_version_compatible?`.

/// Supported spellings: `"9.x"` (major only), `">= 9.0"`, `"~> 9.2"`
/// (pessimistic — same major.minor prefix, at least that patch), or a bare
/// prefix match (mxrb's fallback: `project_version.start_with?(required)`,
/// with a trailing `.*` stripped first).
pub fn compatible(project_version: &str, required: &str) -> bool {
    let project: Vec<i64> = numeric_parts(project_version);
    let required = required.trim();

    if let Some(major) = required.strip_suffix(".x") {
        return major.parse::<i64>().ok() == project.first().copied();
    }
    if let Some(minimum) = required.strip_prefix(">=") {
        return at_least(&project, &numeric_parts(minimum.trim()));
    }
    if let Some(minimum) = required.strip_prefix("~>") {
        let base = numeric_parts(minimum.trim());
        if base.is_empty() {
            return false;
        }
        let prefix_length = base.len().saturating_sub(1);
        return at_least(&project, &base)
            && project[..prefix_length.min(project.len())] == base[..prefix_length];
    }
    let prefix = required.strip_suffix(".*").unwrap_or(required);
    project_version.starts_with(prefix)
}

fn numeric_parts(value: &str) -> Vec<i64> {
    value
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn at_least(actual: &[i64], minimum: &[i64]) -> bool {
    let length = actual.len().max(minimum.len());
    for index in 0..length {
        let a = actual.get(index).copied().unwrap_or(0);
        let b = minimum.get(index).copied().unwrap_or(0);
        if a > b {
            return true;
        }
        if a < b {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_documented_constraint_spelling_matches_mxrb() {
        assert!(compatible("9.12.0", "9.x"));
        assert!(!compatible("10.0.0", "9.x"));

        assert!(compatible("9.12.0", ">= 9.0"));
        assert!(compatible("10.0.0", ">= 9.0"));
        assert!(!compatible("8.6.0", ">= 9.0"));

        assert!(compatible("9.2.5", "~> 9.2"));
        assert!(compatible("9.2.0", "~> 9.2"));
        assert!(!compatible("9.1.9", "~> 9.2"));
        // mxrb's own `~>` slices `base.size - 1` elements for the prefix
        // check — with a two-part base (`9.2`) that is ONE element (just
        // the major), so `~> 9.2` behaves as ">= 9.2 same major", not the
        // conventional "same major.minor" pessimistic operator. Ported
        // faithfully rather than "fixed".
        assert!(compatible("9.3.0", "~> 9.2"));
        assert!(!compatible("10.0.0", "~> 9.2"));
        assert!(
            compatible("10.24.0", ">= 9.0"),
            "text ordering must not fool numeric comparison"
        );

        assert!(compatible("9.12.1", "9.12"));
        assert!(compatible("9.12.1", "9.12.*"));
        assert!(!compatible("9.13.0", "9.12"));
    }
}
