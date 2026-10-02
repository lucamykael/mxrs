//! How the frontend's TypeScript names what the model names: one rule for
//! the importer that writes it, the scaffold that adds to it and the reader.

/// The identifier TypeScript writes a Mendix name with: lower camel case,
/// a leading acronym lowered whole (`SPCProgram` → `spcProgram`).
pub fn camel(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let run = chars.iter().take_while(|c| c.is_ascii_uppercase()).count();
    let lowered = if run == 0 {
        0
    } else if run == chars.len() {
        run
    } else if run > 1 && chars[run].is_ascii_lowercase() {
        run - 1
    } else {
        run.max(1)
    };
    chars
        .iter()
        .enumerate()
        .map(|(index, c)| {
            if index < lowered {
                c.to_ascii_lowercase()
            } else {
                *c
            }
        })
        .collect()
}

/// The Mendix name an identifier reads as when nothing says otherwise.
pub fn pascal(ident: &str) -> String {
    let mut chars = ident.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// A snake-case name in lower camel case: `find_by` → `findBy`.
pub fn camel_from_snake(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = !out.is_empty();
        } else if upper {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// A service's file: `MeasurementUnitService` → `measurementUnitService.ts`.
pub fn service_file(service: &str) -> String {
    format!("{}.ts", camel(service))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_as_camel_case_and_back() {
        for (mendix, ident) in [
            ("MeasurementUnitHelper", "measurementUnitHelper"),
            ("SPCProgram", "spcProgram"),
            ("ID", "id"),
            ("pagination", "pagination"),
            ("Variable_2", "variable_2"),
        ] {
            assert_eq!(camel(mendix), ident, "{mendix}");
        }
        assert_eq!(pascal("measurementUnitHelper"), "MeasurementUnitHelper");
        assert_ne!(pascal(&camel("SPCProgram")), "SPCProgram");
        assert_eq!(camel_from_snake("check_in_use"), "checkInUse");
        assert_eq!(service_file("AssetTypeService"), "assetTypeService.ts");
    }
}
