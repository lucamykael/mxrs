//! Mendix's array-marker convention.
//!
//! Ports `Mxrb::IO::BsonCodec.parse_array` / `.build_array` from
//! `lib/mxrb/io/bson_codec.rb`. Mendix arrays carry an int32 marker as their
//! first element: `1` = by-name references, `2` = part-secondary, `3` =
//! part-primary.

use bson::Bson;

/// A Mendix array split into its leading marker and the actual items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArray {
    pub marker: i32,
    pub items: Vec<Bson>,
}

fn as_marker(value: &Bson) -> Option<i32> {
    match value {
        Bson::Int32(i) => Some(*i),
        Bson::Int64(i) => i32::try_from(*i).ok(),
        _ => None,
    }
}

/// Parses a raw Mendix array. An empty or absent array defaults to marker
/// `3` (part-primary) with no items, matching `bson_codec.rb`'s behavior for
/// `nil`/`[]` input.
pub fn parse_array(raw: Option<&[Bson]>) -> ParsedArray {
    let Some(raw) = raw else {
        return ParsedArray { marker: 3, items: vec![] };
    };
    if raw.is_empty() {
        return ParsedArray { marker: 3, items: vec![] };
    }

    match as_marker(&raw[0]) {
        Some(marker) => ParsedArray {
            marker,
            items: raw[1..].to_vec(),
        },
        None => ParsedArray {
            marker: 3,
            items: raw.to_vec(),
        },
    }
}

/// Builds a Mendix-style array by prepending the marker.
pub fn build_array(items: Vec<Bson>, marker: i32) -> Vec<Bson> {
    let mut result = Vec::with_capacity(items.len() + 1);
    result.push(Bson::Int32(marker));
    result.extend(items);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_by_name_marker() {
        let raw = vec![Bson::Int32(1), Bson::String("Sales.Order".into())];
        let parsed = parse_array(Some(&raw));
        assert_eq!(parsed.marker, 1);
        assert_eq!(parsed.items, vec![Bson::String("Sales.Order".into())]);
    }

    #[test]
    fn defaults_to_marker_3_for_nil_or_empty() {
        assert_eq!(parse_array(None), ParsedArray { marker: 3, items: vec![] });
        assert_eq!(parse_array(Some(&[])), ParsedArray { marker: 3, items: vec![] });
    }

    #[test]
    fn defaults_to_marker_3_when_first_element_is_not_a_marker() {
        let raw = vec![Bson::String("not-a-marker".into())];
        let parsed = parse_array(Some(&raw));
        assert_eq!(parsed.marker, 3);
        assert_eq!(parsed.items, raw);
    }

    #[test]
    fn build_array_round_trips_through_parse_array() {
        let items = vec![Bson::String("a".into()), Bson::String("b".into())];
        let built = build_array(items.clone(), 2);
        let parsed = parse_array(Some(&built));
        assert_eq!(parsed.marker, 2);
        assert_eq!(parsed.items, items);
    }
}
