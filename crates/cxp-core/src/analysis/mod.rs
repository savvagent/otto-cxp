//! Pure customer-experience analytics: Net Promoter Score, Customer
//! Satisfaction, and keyword sentiment, with no database access.
//!
//! The `calculate_nps` and `calculate_csat` tools do not call these: they
//! count in SQL (`FeedbackExt::survey_nps_counts`, `csat_counts`), so no
//! list of scores is ever loaded. [`calculate_nps`] and [`calculate_csat`]
//! are the reference formulas those counts' `nps()`/`csat()` reproduce.
//! [`analyze_sentiment`] backs the `analyze_sentiment` tool directly.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nps_of_nothing_is_undefined() {
        assert_eq!(calculate_nps(&[]), None);
    }

    #[test]
    fn nps_is_promoters_minus_detractors_as_a_percentage() {
        // 9-10 promote, 7-8 are passive, 0-6 detract.
        assert_eq!(calculate_nps(&[10, 9, 8, 7, 6, 0]), Some(0.0));
        assert_eq!(calculate_nps(&[10, 10, 9, 9]), Some(100.0));
        assert_eq!(calculate_nps(&[0, 3, 6, 6]), Some(-100.0));
        assert_eq!(calculate_nps(&[7, 8]), Some(0.0));
        assert_eq!(calculate_nps(&[10, 9, 9, 2]), Some(50.0));
    }

    #[test]
    fn csat_of_nothing_is_undefined() {
        assert_eq!(calculate_csat(&[]), None);
    }

    #[test]
    fn csat_is_the_share_of_fours_and_fives() {
        assert_eq!(calculate_csat(&[5, 4, 3, 2]), Some(50.0));
        assert_eq!(calculate_csat(&[5, 5]), Some(100.0));
        assert_eq!(calculate_csat(&[1, 2, 3]), Some(0.0));
        assert_eq!(calculate_csat(&[4]), Some(100.0));
    }
}
