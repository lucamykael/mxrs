//! Text similarity over the index: each artifact, and a query, as a
//! hashed term-frequency vector, ranked by cosine distance.
//!
//! The vector is mxrb's `Semantic::TfidfEmbedder`, so the same model ranks
//! the same way under both: lowercase ASCII words of two characters or more,
//! each counted into one of 512 buckets by its 32-bit FNV-1a hash, weighted
//! by its share of the words and scaled to unit length. Sums are compensated
//! as Ruby's `Array#sum` compensates them, so distances agree to the last
//! digit printed.

use crate::documents::Artifact;

/// How many buckets a text's words are counted into.
pub const DIMENSION: usize = 512;

/// A text as a unit vector of its words' shares; all zeros without words.
pub fn embed(text: &str) -> Vec<f64> {
    let lowered = text.to_lowercase();
    let mut tokens: Vec<&str> = Vec::new();
    let bytes = lowered.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_lowercase() {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_lowercase() || bytes[index].is_ascii_digit())
            {
                index += 1;
            }
            if index - start >= 2 {
                tokens.push(&lowered[start..index]);
            }
        } else {
            index += 1;
        }
    }
    let mut vector = vec![0.0; DIMENSION];
    if tokens.is_empty() {
        return vector;
    }
    // Counted in the order words first appear, as Ruby's `tally` keeps them.
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for token in &tokens {
        match counts.iter_mut().find(|(word, _)| word == token) {
            Some((_, count)) => *count += 1,
            None => counts.push((token, 1)),
        }
    }
    let total = tokens.len() as f64;
    for (word, count) in counts {
        vector[bucket(word)] += count as f64 / total;
    }
    let norm = compensated_sum(vector.iter().map(|value| value * value)).sqrt();
    if norm == 0.0 {
        return vector;
    }
    vector.iter().map(|value| value / norm).collect()
}

/// What an artifact is searched by: its names, kind, module and
/// documentation, as mxrb's `Embedder.artifact_text` joins them — what it
/// lacks left out.
pub fn artifact_text(artifact: &Artifact) -> String {
    [
        Some(artifact.qualified_name.as_str()),
        artifact.name.as_deref(),
        Some(artifact.kind.as_str()),
        artifact.module_name.as_deref(),
        artifact.metadata["documentation"].as_str(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
}

/// The cosine similarity of two unit vectors.
pub fn similarity(left: &[f64], right: &[f64]) -> f64 {
    compensated_sum(left.iter().zip(right).map(|(left, right)| left * right))
}

fn bucket(word: &str) -> usize {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in word.bytes() {
        hash = (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193);
    }
    hash as usize % DIMENSION
}

/// Kahan–Babuška summation, which Ruby's `sum` uses over floats.
fn compensated_sum(values: impl Iterator<Item = f64>) -> f64 {
    let mut sum = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let total = sum + value;
        if sum.abs() >= value.abs() {
            compensation += (sum - total) + value;
        } else {
            compensation += (value - total) + sum;
        }
        sum = total;
    }
    sum + compensation
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Words are lowercase ASCII runs of two or more, hashed as mxrb hashes
    /// them; a text without one is no direction at all.
    #[test]
    fn a_text_is_the_unit_vector_of_its_words() {
        assert_eq!(bucket("order"), 0x732c_1097_u32 as usize % DIMENSION);
        assert!(embed("a 1 _").iter().all(|value| *value == 0.0));
        let order = embed("Order order ORDER");
        assert_eq!(order[bucket("order")], 1.0);
        let mixed = embed("Sales.Order_Line x");
        let words = ["sales", "order", "line"];
        for word in words {
            assert!((mixed[bucket(word)] - 1.0 / 3.0_f64.sqrt()).abs() < 1e-12);
        }
        assert!((similarity(&mixed, &mixed) - 1.0).abs() < 1e-12);
    }
}
