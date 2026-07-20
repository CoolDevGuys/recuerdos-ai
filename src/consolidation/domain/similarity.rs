//! Cosine similarity between two embeddings.
//!
//! Consolidation needs a number it can compare against a configured
//! threshold, which rules out reusing recall: hybrid search returns
//! *ranks*, deliberately discarding the scores (see
//! `memories/domain/recall_ranker.rs`), and "rank 3 of 8" says nothing
//! about whether two memories say the same thing.
//!
//! So the job re-embeds and compares directly. One number, one scale,
//! one meaning — which is what makes `similarity_threshold = 0.92` a
//! setting an operator can reason about rather than a magic constant.

/// Cosine similarity, in `-1.0..=1.0`.
///
/// Zero for mismatched or empty inputs, and for a zero vector. Returning
/// zero rather than erroring is deliberate: this runs over thousands of
/// pairs in a background job, and one degenerate embedding should mean
/// "these are not similar" rather than abort the night's consolidation.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for (left, right) in a.iter().zip(b) {
        dot += left * right;
        norm_a += left * left;
        norm_b += right * right;
    }

    let magnitude = (norm_a.sqrt()) * (norm_b.sqrt());
    if magnitude == 0.0 || !magnitude.is_finite() {
        return 0.0;
    }

    let similarity = dot / magnitude;
    if similarity.is_finite() {
        // Floating-point error can push an identical pair a hair past
        // 1.0, which would read as "more identical than identical" to
        // anything comparing against a threshold.
        similarity.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_are_one() {
        let vector = [0.3, -0.5, 0.8];
        assert!((cosine(&vector, &vector) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn opposite_vectors_are_minus_one() {
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_are_zero() {
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn magnitude_does_not_matter_only_direction() {
        // The property the threshold depends on: a longer sentence
        // embedding must not read as less similar for being longer.
        assert!((cosine(&[1.0, 2.0], &[10.0, 20.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn degenerate_inputs_are_not_similar_rather_than_fatal() {
        // A nightly job over thousands of pairs must not abort because
        // one embedding came back empty.
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0, 2.0], &[1.0]), 0.0, "mismatched lengths");
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0, "a zero vector");
        assert_eq!(cosine(&[f32::NAN, 1.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[f32::INFINITY, 1.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn the_result_never_escapes_the_valid_range() {
        // Rounding can nudge an identical pair past 1.0, which would read
        // as "more identical than identical" at the threshold.
        let vector: Vec<f32> = (0..384).map(|i| (i as f32) * 0.01).collect();
        let similarity = cosine(&vector, &vector);

        assert!(
            (-1.0..=1.0).contains(&similarity),
            "got {similarity}, outside the valid range"
        );
    }

    #[test]
    fn near_duplicates_score_above_the_default_threshold() {
        // Sanity-checks that 0.92 is a plausible default rather than an
        // arbitrary one: vectors this close are what two phrasings of one
        // preference look like.
        let base: Vec<f32> = (0..64).map(|i| ((i % 7) as f32) - 3.0).collect();
        let nudged: Vec<f32> = base.iter().map(|value| value + 0.15).collect();

        assert!(cosine(&base, &nudged) > 0.92, "{}", cosine(&base, &nudged));
    }
}
