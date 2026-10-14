//! Feedback, surveys, and survey responses: `FeedbackExt` on
//! `otto_tenant::Tx`, with its model structs.
//!
//! Customer ids arrive already resolved. Every statement binds the org
//! explicitly as well as relying on row-level security, and the composite
//! `(org_id, x_id)` foreign keys refuse another org's customer, survey, or
//! catalog row exactly as they refuse a missing one. Nothing here meters; the
//! billable tools charge in the layer above.

use chrono::{DateTime, Utc};
use otto_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{map_fk, Error, Result};
use crate::paging::Page;

/// The values `feedback_type` accepts.
pub const FEEDBACK_TYPES: [&str; 5] = [
    "bug_report",
    "feature_request",
    "complaint",
    "praise",
    "suggestion",
];

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Feedback {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub feedback_type: String,
    pub subject: Option<String>,
    pub body: Option<String>,
    pub rating: Option<i32>,
    pub metadata: serde_json::Value,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Survey {
    pub id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub questions: serde_json::Value,
    pub active: bool,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SurveyResponse {
    pub id: Uuid,
    pub survey_id: Uuid,
    pub customer_id: Uuid,
    pub answers: serde_json::Value,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// A feedback entry to record. `rating`, when given, is 1 to 5.
#[derive(Debug, Clone, Default)]
pub struct NewFeedback {
    pub customer_id: Uuid,
    pub feedback_type: String,
    pub subject: Option<String>,
    pub body: Option<String>,
    pub rating: Option<i32>,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// Filters for [`FeedbackExt::list_feedback`]; `None` means "any".
#[derive(Debug, Clone, Default)]
pub struct FeedbackFilter {
    pub customer_id: Option<Uuid>,
    pub feedback_type: Option<String>,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// A survey to create. `questions` must be a JSON array.
#[derive(Debug, Clone, Default)]
pub struct NewSurvey {
    pub title: String,
    pub description: Option<String>,
    pub questions: serde_json::Value,
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// Fields to change on a survey; `None` leaves a field as it is. A field
/// cannot be cleared back to empty (as before the port).
#[derive(Debug, Clone, Default)]
pub struct SurveyUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub questions: Option<serde_json::Value>,
    pub active: Option<bool>,
}

/// Filters for [`FeedbackExt::list_surveys`]; `None` means "any".
#[derive(Debug, Clone, Default)]
pub struct SurveyFilter {
    pub experience_id: Option<Uuid>,
    pub application_id: Option<Uuid>,
    pub page_id: Option<Uuid>,
    pub component_id: Option<Uuid>,
}

/// The NPS inputs for one survey, aggregated in SQL. A response's score is
/// the first answer's `value` when `answers` is an array (an integer or an
/// integer string) or the `score` key when it is an object (an integer
/// number only; a string is unscored). Scores outside 0 to 10 and responses
/// with no score do not count as scored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NpsCounts {
    pub total_responses: i64,
    pub scored_responses: i64,
    pub promoters: i64,
    pub passives: i64,
    pub detractors: i64,
}

impl NpsCounts {
    /// Net Promoter Score, `None` when nothing is scored. Same formula as
    /// [`crate::analysis::calculate_nps`], over counts instead of a list.
    pub fn nps(&self) -> Option<f64> {
        (self.scored_responses > 0).then(|| {
            (self.promoters - self.detractors) as f64 / self.scored_responses as f64 * 100.0
        })
    }
}

/// The CSAT inputs over feedback, aggregated in SQL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CsatCounts {
    pub total_feedback: i64,
    pub rated_feedback: i64,
    pub satisfied: i64,
    pub unsatisfied: i64,
}

impl CsatCounts {
    /// Customer Satisfaction Score, `None` when nothing is rated. Same formula
    /// as [`crate::analysis::calculate_csat`], over counts.
    pub fn csat(&self) -> Option<f64> {
        (self.rated_feedback > 0)
            .then(|| self.satisfied as f64 / self.rated_feedback as f64 * 100.0)
    }
}

const FEEDBACK_COLS: &str = "id, customer_id, feedback_type::text AS feedback_type, subject, \
     body, rating, metadata, experience_id, application_id, page_id, component_id, created_at";

const SURVEY_COLS: &str = "id, title, description, questions, active, experience_id, \
     application_id, page_id, component_id, created_at, updated_at";

const RESPONSE_COLS: &str = "id, survey_id, customer_id, answers, completed_at, created_at";

fn check_feedback_type(t: &str) -> Result<()> {
    if FEEDBACK_TYPES.contains(&t) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "feedback type must be one of {}",
            FEEDBACK_TYPES.join(", ")
        )))
    }
}

fn check_questions(q: &serde_json::Value) -> Result<()> {
    if q.is_array() {
        Ok(())
    } else {
        Err(Error::Invalid(
            "questions must be a JSON array of question objects".into(),
        ))
    }
}

fn check_answers(a: &serde_json::Value) -> Result<()> {
    if a.is_array() || a.is_object() {
        Ok(())
    } else {
        Err(Error::Invalid(
            "answers must be a JSON array or object".into(),
        ))
    }
}

/// The row kind a foreign-key violation names, by the exact generated
/// constraint names of `feedback`, `surveys`, and `survey_responses` (see
/// [`map_fk`]).
const FK_KINDS: &[(&str, &str)] = &[
    ("feedback_org_id_customer_id_fkey", "customer"),
    ("survey_responses_org_id_customer_id_fkey", "customer"),
    ("survey_responses_org_id_survey_id_fkey", "survey"),
    ("feedback_org_id_experience_id_fkey", "experience"),
    ("surveys_org_id_experience_id_fkey", "experience"),
    ("feedback_org_id_application_id_fkey", "application"),
    ("surveys_org_id_application_id_fkey", "application"),
    ("feedback_org_id_page_id_fkey", "page"),
    ("surveys_org_id_page_id_fkey", "page"),
    ("feedback_org_id_component_id_fkey", "component"),
    ("surveys_org_id_component_id_fkey", "component"),
];

const FEEDBACK_WHERE: &str = "org_id = $1 \
     AND ($2::uuid IS NULL OR customer_id = $2) \
     AND ($3::text IS NULL OR feedback_type = $3::feedback_type) \
     AND ($4::uuid IS NULL OR experience_id = $4) \
     AND ($5::uuid IS NULL OR application_id = $5) \
     AND ($6::uuid IS NULL OR page_id = $6) \
     AND ($7::uuid IS NULL OR component_id = $7)";

const SURVEY_WHERE: &str = "org_id = $1 \
     AND ($2::uuid IS NULL OR experience_id = $2) \
     AND ($3::uuid IS NULL OR application_id = $3) \
     AND ($4::uuid IS NULL OR page_id = $4) \
     AND ($5::uuid IS NULL OR component_id = $5)";

pub trait FeedbackExt {
    /// Record feedback. A bad type or rating is `Invalid`; another org's (or a
    /// missing) customer or catalog row is `NotFound`.
    fn submit_feedback(
        &mut self,
        new: &NewFeedback,
    ) -> impl std::future::Future<Output = Result<Feedback>> + Send;

    /// Feedback newest first, and the total matching the filter.
    fn list_feedback(
        &mut self,
        filter: &FeedbackFilter,
        page: Page,
    ) -> impl std::future::Future<Output = Result<(Vec<Feedback>, i64)>> + Send;

    /// CSAT inputs over all feedback, or one `feedback_type`, as SQL counts.
    fn feedback_csat_counts(
        &mut self,
        feedback_type: Option<&str>,
    ) -> impl std::future::Future<Output = Result<CsatCounts>> + Send;

    /// Create an active survey. An empty title or non-array `questions` is
    /// `Invalid`; an unknown catalog id is `NotFound` of that kind.
    fn create_survey(
        &mut self,
        new: &NewSurvey,
    ) -> impl std::future::Future<Output = Result<Survey>> + Send;

    /// `NotFound` when the survey is not in this organization.
    fn get_survey(&mut self, id: Uuid) -> impl std::future::Future<Output = Result<Survey>> + Send;

    /// Surveys newest first, and the total matching the filter.
    fn list_surveys(
        &mut self,
        filter: &SurveyFilter,
        page: Page,
    ) -> impl std::future::Future<Output = Result<(Vec<Survey>, i64)>> + Send;

    /// Change the named fields. An empty title or non-array `questions` is
    /// `Invalid`; `NotFound` when the survey is not in this organization.
    fn update_survey(
        &mut self,
        id: Uuid,
        update: &SurveyUpdate,
    ) -> impl std::future::Future<Output = Result<Survey>> + Send;

    /// Delete a survey and, by cascade, its responses.
    fn delete_survey(&mut self, id: Uuid) -> impl std::future::Future<Output = Result<()>> + Send;

    /// Send a survey to a customer: an open response with no answers yet.
    fn send_survey(
        &mut self,
        survey_id: Uuid,
        customer_id: Uuid,
    ) -> impl std::future::Future<Output = Result<SurveyResponse>> + Send;

    /// Record a customer's completed response in one statement: the row is
    /// created with its answers and `completed_at` together.
    fn submit_survey_response(
        &mut self,
        survey_id: Uuid,
        customer_id: Uuid,
        answers: &serde_json::Value,
    ) -> impl std::future::Future<Output = Result<SurveyResponse>> + Send;

    /// A survey's responses newest first, and the total. `NotFound` when the
    /// survey is not in this organization.
    fn list_survey_responses(
        &mut self,
        survey_id: Uuid,
        page: Page,
    ) -> impl std::future::Future<Output = Result<(Vec<SurveyResponse>, i64)>> + Send;

    /// NPS inputs for a survey as SQL counts. `NotFound` when the survey is
    /// not in this organization.
    fn survey_nps_counts(
        &mut self,
        survey_id: Uuid,
    ) -> impl std::future::Future<Output = Result<NpsCounts>> + Send;
}

impl FeedbackExt for Tx<'_> {
    async fn submit_feedback(&mut self, new: &NewFeedback) -> Result<Feedback> {
        check_feedback_type(&new.feedback_type)?;
        if let Some(r) = new.rating {
            if !(1..=5).contains(&r) {
                return Err(Error::Invalid(format!(
                    "rating must be between 1 and 5; got {r}"
                )));
            }
        }
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO feedback (org_id, customer_id, feedback_type, subject, body, rating, \
             experience_id, application_id, page_id, component_id) \
             VALUES ($1, $2, $3::feedback_type, $4, $5, $6, $7, $8, $9, $10) \
             RETURNING {FEEDBACK_COLS}"
        ))
        .bind(org)
        .bind(new.customer_id)
        .bind(&new.feedback_type)
        .bind(&new.subject)
        .bind(&new.body)
        .bind(new.rating)
        .bind(new.experience_id)
        .bind(new.application_id)
        .bind(new.page_id)
        .bind(new.component_id)
        .fetch_one(self.conn())
        .await
        .map_err(|e| map_fk(e, FK_KINDS))
    }

    async fn list_feedback(
        &mut self,
        f: &FeedbackFilter,
        page: Page,
    ) -> Result<(Vec<Feedback>, i64)> {
        if let Some(t) = &f.feedback_type {
            check_feedback_type(t)?;
        }
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {FEEDBACK_COLS} FROM feedback WHERE {FEEDBACK_WHERE} \
             ORDER BY created_at DESC, id LIMIT $8 OFFSET $9"
        ))
        .bind(org)
        .bind(f.customer_id)
        .bind(&f.feedback_type)
        .bind(f.experience_id)
        .bind(f.application_id)
        .bind(f.page_id)
        .bind(f.component_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let total: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM feedback WHERE {FEEDBACK_WHERE}"
        ))
        .bind(org)
        .bind(f.customer_id)
        .bind(&f.feedback_type)
        .bind(f.experience_id)
        .bind(f.application_id)
        .bind(f.page_id)
        .bind(f.component_id)
        .fetch_one(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn feedback_csat_counts(&mut self, feedback_type: Option<&str>) -> Result<CsatCounts> {
        if let Some(t) = feedback_type {
            check_feedback_type(t)?;
        }
        let org = self.org();
        let (total, rated, satisfied): (i64, i64, i64) = sqlx::query_as(
            "SELECT count(*), count(rating), count(*) FILTER (WHERE rating >= 4) \
             FROM feedback \
             WHERE org_id = $1 AND ($2::text IS NULL OR feedback_type = $2::feedback_type)",
        )
        .bind(org)
        .bind(feedback_type)
        .fetch_one(self.conn())
        .await?;
        Ok(CsatCounts {
            total_feedback: total,
            rated_feedback: rated,
            satisfied,
            unsatisfied: rated - satisfied,
        })
    }

    async fn create_survey(&mut self, new: &NewSurvey) -> Result<Survey> {
        if new.title.trim().is_empty() {
            return Err(Error::Invalid("survey title must not be empty".into()));
        }
        check_questions(&new.questions)?;
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO surveys (org_id, title, description, questions, experience_id, \
             application_id, page_id, component_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING {SURVEY_COLS}"
        ))
        .bind(org)
        .bind(&new.title)
        .bind(&new.description)
        .bind(&new.questions)
        .bind(new.experience_id)
        .bind(new.application_id)
        .bind(new.page_id)
        .bind(new.component_id)
        .fetch_one(self.conn())
        .await
        .map_err(|e| map_fk(e, FK_KINDS))
    }

    async fn get_survey(&mut self, id: Uuid) -> Result<Survey> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {SURVEY_COLS} FROM surveys WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "survey" })
    }

    async fn list_surveys(&mut self, f: &SurveyFilter, page: Page) -> Result<(Vec<Survey>, i64)> {
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {SURVEY_COLS} FROM surveys WHERE {SURVEY_WHERE} \
             ORDER BY created_at DESC, id LIMIT $6 OFFSET $7"
        ))
        .bind(org)
        .bind(f.experience_id)
        .bind(f.application_id)
        .bind(f.page_id)
        .bind(f.component_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let total: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM surveys WHERE {SURVEY_WHERE}"
        ))
        .bind(org)
        .bind(f.experience_id)
        .bind(f.application_id)
        .bind(f.page_id)
        .bind(f.component_id)
        .fetch_one(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn update_survey(&mut self, id: Uuid, u: &SurveyUpdate) -> Result<Survey> {
        if let Some(t) = &u.title {
            if t.trim().is_empty() {
                return Err(Error::Invalid("survey title must not be empty".into()));
            }
        }
        if let Some(q) = &u.questions {
            check_questions(q)?;
        }
        let org = self.org();
        sqlx::query_as(&format!(
            "UPDATE surveys SET title = COALESCE($3, title), \
             description = COALESCE($4, description), \
             questions = COALESCE($5, questions), active = COALESCE($6, active), \
             updated_at = now() \
             WHERE org_id = $1 AND id = $2 RETURNING {SURVEY_COLS}"
        ))
        .bind(org)
        .bind(id)
        .bind(&u.title)
        .bind(&u.description)
        .bind(&u.questions)
        .bind(u.active)
        .fetch_optional(self.conn())
        .await?
        .ok_or(Error::NotFound { kind: "survey" })
    }

    async fn delete_survey(&mut self, id: Uuid) -> Result<()> {
        let org = self.org();
        let done = sqlx::query("DELETE FROM surveys WHERE org_id = $1 AND id = $2")
            .bind(org)
            .bind(id)
            .execute(self.conn())
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound { kind: "survey" });
        }
        Ok(())
    }

    async fn send_survey(&mut self, survey_id: Uuid, customer_id: Uuid) -> Result<SurveyResponse> {
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO survey_responses (org_id, survey_id, customer_id, answers) \
             VALUES ($1, $2, $3, '{{}}'::jsonb) RETURNING {RESPONSE_COLS}"
        ))
        .bind(org)
        .bind(survey_id)
        .bind(customer_id)
        .fetch_one(self.conn())
        .await
        .map_err(|e| map_fk(e, FK_KINDS))
    }

    async fn submit_survey_response(
        &mut self,
        survey_id: Uuid,
        customer_id: Uuid,
        answers: &serde_json::Value,
    ) -> Result<SurveyResponse> {
        check_answers(answers)?;
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO survey_responses (org_id, survey_id, customer_id, answers, completed_at) \
             VALUES ($1, $2, $3, $4, now()) RETURNING {RESPONSE_COLS}"
        ))
        .bind(org)
        .bind(survey_id)
        .bind(customer_id)
        .bind(answers)
        .fetch_one(self.conn())
        .await
        .map_err(|e| map_fk(e, FK_KINDS))
    }

    async fn list_survey_responses(
        &mut self,
        survey_id: Uuid,
        page: Page,
    ) -> Result<(Vec<SurveyResponse>, i64)> {
        self.get_survey(survey_id).await?;
        let org = self.org();
        let rows = sqlx::query_as(&format!(
            "SELECT {RESPONSE_COLS} FROM survey_responses WHERE org_id = $1 AND survey_id = $2 \
             ORDER BY created_at DESC, id LIMIT $3 OFFSET $4"
        ))
        .bind(org)
        .bind(survey_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        let total: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM survey_responses WHERE org_id = $1 AND survey_id = $2",
        )
        .bind(org)
        .bind(survey_id)
        .fetch_one(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn survey_nps_counts(&mut self, survey_id: Uuid) -> Result<NpsCounts> {
        self.get_survey(survey_id).await?;
        let org = self.org();
        // The score is `answers[0].value` (integer or integer string) for an
        // array, `answers.score` (integer number only) for an object.
        let (total, scored, promoters, passives, detractors): (i64, i64, i64, i64, i64) =
            sqlx::query_as(
                "WITH raw AS ( \
                   SELECT CASE jsonb_typeof(answers) \
                            WHEN 'array' THEN answers -> 0 -> 'value' \
                            WHEN 'object' THEN answers -> 'score' END AS v, \
                          jsonb_typeof(answers) = 'array' AS is_array \
                   FROM survey_responses WHERE org_id = $1 AND survey_id = $2), \
                 txt AS ( \
                   SELECT CASE WHEN jsonb_typeof(v) = 'number' \
                                 OR (jsonb_typeof(v) = 'string' AND is_array) \
                               THEN v #>> '{}' END AS t \
                   FROM raw), \
                 scores AS ( \
                   SELECT t::int AS s FROM txt WHERE t ~ '^[+-]?[0-9]{1,9}$') \
                 SELECT (SELECT count(*) FROM raw), \
                        count(*) FILTER (WHERE s BETWEEN 0 AND 10), \
                        count(*) FILTER (WHERE s BETWEEN 9 AND 10), \
                        count(*) FILTER (WHERE s BETWEEN 7 AND 8), \
                        count(*) FILTER (WHERE s BETWEEN 0 AND 6) \
                 FROM scores",
            )
            .bind(org)
            .bind(survey_id)
            .fetch_one(self.conn())
            .await?;
        Ok(NpsCounts {
            total_responses: total,
            scored_responses: scored,
            promoters,
            passives,
            detractors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_constraint_names_its_kind() {
        crate::error::fake::assert_kinds(FK_KINDS);
    }
}
