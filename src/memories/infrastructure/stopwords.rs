//! The keyword leg's stopword list — the words a BM25 match on which is
//! *not* evidence of relevance.
//!
//! # Why this is a project-owned list and not tantivy's built-in one
//!
//! tantivy's English list (the Lucene one: `a, an, and, are, …`) targets
//! the articles, prepositions and conjunctions of *prose*. The queries
//! this service answers are not prose, they are natural-language
//! questions from agents — "what do I use for data storage", "how should
//! I organise imports" — and none of `what, do, how, should, I` is in
//! tantivy's list. With only that list, each of those words matches any
//! memory containing it, and RRF promotes whatever generic memories
//! happen to collect such matches (reranking-plan.md §1). So the list
//! below is tantivy's list **plus** the words natural questions are made
//! of: question words, auxiliaries and modals, pronouns and
//! determiners, and a handful of connective glue.
//!
//! # What is deliberately *not* here
//!
//! Domain words that carry signal in this corpus — `use`, `user`,
//! `prefer` — are kept despite being frequent, because dropping them
//! drops the query's subject, not its filler. `not` and `no` *are* on
//! the list (they come with tantivy's and matching BM25 on a negation is
//! pure noise), which is a real trade: negation recall falls onto the
//! vector leg, which handles it well. Each entry is a trade-off like
//! that one; the `negative` and `exact-token` eval kinds guard the
//! boundary, so a change here should arrive with those numbers moving.
//!
//! The list applies symmetrically — to indexed text and to queries
//! (the query parser uses each field's tokenizer, see
//! `tantivy_text_index`). A word absent from memory text and from every
//! query costs nothing; a word in neither place is simply dead weight.

/// Lowercase — the filter runs after the `LowerCaser`, and tantivy's
/// `StopWordFilter` compares whole tokens against this set.
///
/// The list, in the order it was assembled: first tantivy/Lucene's
/// English words, then question words, auxiliaries and modals,
/// pronouns and determiners, and connective glue — see the module docs
/// for why each group is here and why the domain words are not.
#[rustfmt::skip]
pub const STOP_WORDS: &[&str] = &[
    // tantivy / Lucene's English list.
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it",
    "no", "not", "of", "on", "or", "such", "that", "the", "their", "then", "there", "these",
    "they", "this", "to", "was", "will", "with",
    // Question words — what agent queries are built from.
    "how", "what", "when", "where", "which", "who", "whom", "whose", "why",
    // Auxiliaries and modals.
    "am", "been", "being", "could", "did", "do", "does", "had", "has", "have", "may", "might",
    "must", "shall", "should", "were", "would",
    // Pronouns and determiners.
    "he", "her", "him", "his", "i", "its", "me", "mine", "our", "she", "them", "those", "we",
    "you", "your",
    // Connective glue.
    "about", "all", "any", "from", "over", "some", "so", "than", "under",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_entry_is_lowercase_and_unique() {
        let mut seen = HashSet::new();
        for word in STOP_WORDS {
            assert_eq!(*word, word.to_lowercase(), "{word} is not lowercase");
            assert!(seen.insert(word), "{word} appears twice");
        }
    }

    #[test]
    fn the_question_words_the_built_in_list_lacks_are_here() {
        // The whole reason this file exists (§1 of the reranking plan).
        for word in ["what", "do", "does", "how", "should", "i"] {
            assert!(
                STOP_WORDS.contains(&word),
                "{word} must be a stopword or NL queries match on it"
            );
        }
    }

    #[test]
    fn domain_words_that_carry_signal_stay_out() {
        for word in ["use", "user", "prefer", "database", "deploy"] {
            assert!(
                !STOP_WORDS.contains(&word),
                "{word} carries the subject of the query, not filler"
            );
        }
    }
}
