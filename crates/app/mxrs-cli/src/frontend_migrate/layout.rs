//! Layout-grid rows whose desktop column weights do not add up to twelve.
//!
//! Older models store a proportional column as a negative weight (`-1` is
//! "one share of what is left"); Mendix now checks that a row's desktop
//! `Weight`s are positive and total twelve (CE0535). A row is normalized
//! when the shares can take what the fixed columns leave, each at least
//! one column wide; anything else is an `unsafe_layout_weights` issue.
//! `TabletWeight` and `PhoneWeight` keep `-1`, which there means "inherit".

use mxrs_bson::{Bson, Document};

use super::{IssueKind, UnitScope, tree};

/// The weights a row's columns must total.
const ROW_WIDTH: i128 = 12;

/// Normalizes a `Forms$LayoutGridRow`'s desktop weights in place.
pub(super) fn migrate_row(row: &mut Document, path: &str, scope: &mut UnitScope<'_>) {
    let columns = tree::items_mut(row.get_mut("Columns"));
    if columns.is_empty() {
        return;
    }
    let values: Vec<Option<&Bson>> = columns
        .iter()
        .map(|column| tree::field(column, "Weight"))
        .collect();
    let weights: Option<Vec<i128>> = values
        .iter()
        .map(|value| tree::integer(*value).map(i128::from))
        .collect();
    if weights.as_ref().is_some_and(|weights| {
        weights.iter().all(|weight| *weight > 0) && weights.iter().sum::<i128>() == ROW_WIDTH
    }) {
        return;
    }
    let Some(normalized) = weights.as_deref().and_then(normalize_weights) else {
        let values: Vec<Bson> = values
            .iter()
            .map(|value| value.cloned().unwrap_or(Bson::Null))
            .collect();
        scope.issue(
            path,
            IssueKind::UnsafeLayoutWeights,
            format!(
                "Weight values cannot be normalized safely: {}",
                tree::inspect(Some(&Bson::Array(values)))
            ),
        );
        return;
    };
    for (column, weight) in columns.iter_mut().zip(normalized) {
        if let Bson::Document(column) = column {
            // A normalized weight lies between 1 and 12, or is a fixed
            // weight the row already stored.
            let weight = i64::try_from(weight).expect("a stored weight fits in 64 bits");
            column.insert("Weight", tree::integer_bson(weight));
        }
    }
    scope.counts.layout_rows += 1;
}

/// The weights with every share given its part of what the fixed columns
/// leave, or nothing when that cannot be done losslessly: a zero weight, no
/// share to give the remainder to, fixed columns already at or past the
/// row's width, or a share that would get no column at all.
pub(super) fn normalize_weights(weights: &[i128]) -> Option<Vec<i128>> {
    if weights.contains(&0) {
        return None;
    }
    let fixed: i128 = weights.iter().filter(|weight| **weight > 0).sum();
    let shares: Vec<usize> = (0..weights.len())
        .filter(|index| weights[*index] < 0)
        .collect();
    if fixed > ROW_WIDTH || (fixed >= ROW_WIDTH && !shares.is_empty()) || shares.is_empty() {
        return None;
    }
    let allocated = apportion(
        ROW_WIDTH - fixed,
        &shares
            .iter()
            .map(|index| weights[*index].abs())
            .collect::<Vec<_>>(),
    );
    if allocated.contains(&0) {
        return None;
    }
    let mut normalized = weights.to_vec();
    for (index, columns) in shares.into_iter().zip(allocated) {
        normalized[index] = columns;
    }
    Some(normalized)
}

/// Splits `total` in proportion to `weights` by largest remainder: each
/// gets the whole part of its exact share, and what is left goes one by
/// one to the largest fractions, the earlier first on a tie.
fn apportion(total: i128, weights: &[i128]) -> Vec<i128> {
    let denominator: i128 = weights.iter().sum();
    let mut result: Vec<i128> = weights
        .iter()
        .map(|weight| total * weight / denominator)
        .collect();
    let remainders: Vec<i128> = weights
        .iter()
        .map(|weight| total * weight % denominator)
        .collect();
    let remaining = total - result.iter().sum::<i128>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|index| (std::cmp::Reverse(remainders[*index]), *index));
    for index in order
        .into_iter()
        .take(usize::try_from(remaining).unwrap_or(0))
    {
        result[index] += 1;
    }
    result
}
