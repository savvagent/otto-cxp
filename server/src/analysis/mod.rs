pub mod sentiment;

pub use sentiment::{analyze_sentiment, SentimentResult};

/// Calculate Net Promoter Score from a list of scores (0-10).
/// NPS = %promoters (9-10) - %detractors (0-6)
pub fn calculate_nps(scores: &[i32]) -> Option<f64> {
    if scores.is_empty() {
        return None;
    }
    let total = scores.len() as f64;
    let promoters = scores.iter().filter(|&&s| s >= 9).count() as f64;
    let detractors = scores.iter().filter(|&&s| s <= 6).count() as f64;
    Some(((promoters - detractors) / total) * 100.0)
}

/// Calculate Customer Satisfaction Score from ratings (1-5).
/// CSAT = (count of 4+5 / total) * 100
pub fn calculate_csat(ratings: &[i32]) -> Option<f64> {
    if ratings.is_empty() {
        return None;
    }
    let total = ratings.len() as f64;
    let satisfied = ratings.iter().filter(|&&r| r >= 4).count() as f64;
    Some((satisfied / total) * 100.0)
}
