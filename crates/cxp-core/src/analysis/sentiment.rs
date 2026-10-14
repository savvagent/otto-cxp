//! Keyword-based sentiment analysis (v1): counts positive and negative
//! words and scores their balance from -1.0 to 1.0.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SentimentResult {
    pub score: f64,
    pub label: String,
    pub positive_words: Vec<String>,
    pub negative_words: Vec<String>,
}

const POSITIVE_WORDS: &[&str] = &[
    "great",
    "excellent",
    "amazing",
    "wonderful",
    "fantastic",
    "love",
    "perfect",
    "awesome",
    "outstanding",
    "brilliant",
    "superb",
    "happy",
    "pleased",
    "satisfied",
    "helpful",
    "friendly",
    "fast",
    "easy",
    "intuitive",
    "reliable",
    "impressed",
    "recommend",
    "best",
    "thank",
    "thanks",
    "good",
    "nice",
    "well",
];

const NEGATIVE_WORDS: &[&str] = &[
    "terrible",
    "awful",
    "horrible",
    "worst",
    "hate",
    "broken",
    "bug",
    "slow",
    "frustrating",
    "disappointed",
    "poor",
    "bad",
    "useless",
    "annoying",
    "difficult",
    "complicated",
    "confusing",
    "unreliable",
    "crash",
    "error",
    "fail",
    "failed",
    "problem",
    "issue",
    "wrong",
    "never",
    "waste",
    "unacceptable",
    "ridiculous",
];

/// Keyword-based sentiment analysis (v1).
/// Returns a score from -1.0 (very negative) to 1.0 (very positive).
pub fn analyze_sentiment(text: &str) -> SentimentResult {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    let mut positive_found = Vec::new();
    let mut negative_found = Vec::new();

    for word in &words {
        let cleaned: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
        if POSITIVE_WORDS.contains(&cleaned.as_str()) {
            positive_found.push(cleaned.clone());
        }
        if NEGATIVE_WORDS.contains(&cleaned.as_str()) {
            negative_found.push(cleaned.clone());
        }
    }

    let pos = positive_found.len() as f64;
    let neg = negative_found.len() as f64;
    let total = pos + neg;

    let score = if total == 0.0 {
        0.0
    } else {
        (pos - neg) / total
    };

    let label = if score > 0.25 {
        "positive"
    } else if score < -0.25 {
        "negative"
    } else {
        "neutral"
    }
    .to_string();

    SentimentResult {
        score,
        label,
        positive_words: positive_found,
        negative_words: negative_found,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_keywords_is_neutral_zero() {
        let r = analyze_sentiment("the order arrived on tuesday");
        assert_eq!(r.score, 0.0);
        assert_eq!(r.label, "neutral");
        assert!(r.positive_words.is_empty() && r.negative_words.is_empty());
        assert_eq!(analyze_sentiment("").label, "neutral");
    }

    #[test]
    fn positive_words_score_positive() {
        let r = analyze_sentiment("Great support, really helpful and FAST!");
        assert_eq!(r.score, 1.0);
        assert_eq!(r.label, "positive");
        assert_eq!(r.positive_words, ["great", "helpful", "fast"]);
    }

    #[test]
    fn negative_words_score_negative() {
        let r = analyze_sentiment("The app is slow and broken.");
        assert_eq!(r.score, -1.0);
        assert_eq!(r.label, "negative");
        assert_eq!(r.negative_words, ["slow", "broken"]);
    }

    #[test]
    fn a_balance_inside_the_quarter_band_is_neutral() {
        // One of each: (1 - 1) / 2 = 0.
        let r = analyze_sentiment("good idea, bad execution");
        assert_eq!(r.score, 0.0);
        assert_eq!(r.label, "neutral");
        // Three positive, two negative: 0.2, still neutral.
        let r = analyze_sentiment("good nice great but slow and bad");
        assert!((r.score - 0.2).abs() < 1e-9, "{}", r.score);
        assert_eq!(r.label, "neutral");
        // Two positive, one negative: 1/3, positive.
        let r = analyze_sentiment("good nice but slow");
        assert_eq!(r.label, "positive");
    }

    #[test]
    fn punctuation_is_stripped_before_matching() {
        let r = analyze_sentiment("thanks!!! (excellent)");
        assert_eq!(r.positive_words, ["thanks", "excellent"]);
    }
}
