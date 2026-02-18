use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentimentResult {
    pub score: f64,
    pub label: String,
    pub positive_words: Vec<String>,
    pub negative_words: Vec<String>,
}

const POSITIVE_WORDS: &[&str] = &[
    "great", "excellent", "amazing", "wonderful", "fantastic", "love", "perfect",
    "awesome", "outstanding", "brilliant", "superb", "happy", "pleased", "satisfied",
    "helpful", "friendly", "fast", "easy", "intuitive", "reliable", "impressed",
    "recommend", "best", "thank", "thanks", "good", "nice", "well",
];

const NEGATIVE_WORDS: &[&str] = &[
    "terrible", "awful", "horrible", "worst", "hate", "broken", "bug", "slow",
    "frustrating", "disappointed", "poor", "bad", "useless", "annoying", "difficult",
    "complicated", "confusing", "unreliable", "crash", "error", "fail", "failed",
    "problem", "issue", "wrong", "never", "waste", "unacceptable", "ridiculous",
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
