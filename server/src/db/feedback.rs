use sqlx::PgPool;
use uuid::Uuid;

use crate::models::feedback::{Feedback, Survey, SurveyResponse};

const FEEDBACK_COLS: &str = r#"
    id, customer_id, feedback_type::text as feedback_type,
    subject, body, rating, metadata,
    experience_id, application_id, page_id, component_id,
    created_at
"#;

#[derive(Debug, Clone)]
pub struct FeedbackDb {
    pool: PgPool,
}

impl FeedbackDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn submit_feedback(
        &self,
        customer_id: Uuid,
        feedback_type: &str,
        subject: Option<&str>,
        body: Option<&str>,
        rating: Option<i32>,
        experience_id: Option<Uuid>,
        application_id: Option<Uuid>,
        page_id: Option<Uuid>,
        component_id: Option<Uuid>,
    ) -> Result<Feedback, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        let sql = format!(
            "INSERT INTO feedback (id, customer_id, feedback_type, subject, body, rating, metadata,
                                   experience_id, application_id, page_id, component_id, created_at)
             VALUES ($1, $2, $3::feedback_type, $4, $5, $6, '{{}}', $7, $8, $9, $10, $11)
             RETURNING {FEEDBACK_COLS}"
        );

        sqlx::query_as::<_, Feedback>(&sql)
            .bind(id)
            .bind(customer_id)
            .bind(feedback_type)
            .bind(subject)
            .bind(body)
            .bind(rating)
            .bind(experience_id)
            .bind(application_id)
            .bind(page_id)
            .bind(component_id)
            .bind(now)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn get_feedback(
        &self,
        customer_id: Option<Uuid>,
        feedback_type: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Feedback>, sqlx::Error> {
        let sql = format!(
            "SELECT {FEEDBACK_COLS} FROM feedback
             WHERE ($1::uuid IS NULL OR customer_id = $1)
               AND ($2::text IS NULL OR feedback_type = $2::feedback_type)
             ORDER BY created_at DESC
             LIMIT $3"
        );

        sqlx::query_as::<_, Feedback>(&sql)
            .bind(customer_id)
            .bind(feedback_type)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
    }

    pub async fn list_feedback(
        &self,
        customer_id: Option<Uuid>,
        feedback_type: Option<&str>,
        experience_id: Option<Uuid>,
        application_id: Option<Uuid>,
        page_id: Option<Uuid>,
        component_id: Option<Uuid>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Feedback>, sqlx::Error> {
        let sql = format!(
            "SELECT {FEEDBACK_COLS} FROM feedback
             WHERE ($1::uuid IS NULL OR customer_id = $1)
               AND ($2::text IS NULL OR feedback_type = $2::feedback_type)
               AND ($3::uuid IS NULL OR experience_id = $3)
               AND ($4::uuid IS NULL OR application_id = $4)
               AND ($5::uuid IS NULL OR page_id = $5)
               AND ($6::uuid IS NULL OR component_id = $6)
             ORDER BY created_at DESC
             LIMIT $7 OFFSET $8"
        );

        sqlx::query_as::<_, Feedback>(&sql)
            .bind(customer_id)
            .bind(feedback_type)
            .bind(experience_id)
            .bind(application_id)
            .bind(page_id)
            .bind(component_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await
    }

    pub async fn count_feedback(
        &self,
        feedback_type: Option<&str>,
        experience_id: Option<Uuid>,
        application_id: Option<Uuid>,
        page_id: Option<Uuid>,
        component_id: Option<Uuid>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM feedback
             WHERE ($1::text IS NULL OR feedback_type = $1::feedback_type)
               AND ($2::uuid IS NULL OR experience_id = $2)
               AND ($3::uuid IS NULL OR application_id = $3)
               AND ($4::uuid IS NULL OR page_id = $4)
               AND ($5::uuid IS NULL OR component_id = $5)",
        )
        .bind(feedback_type)
        .bind(experience_id)
        .bind(application_id)
        .bind(page_id)
        .bind(component_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn create_survey(
        &self,
        title: &str,
        description: Option<&str>,
        questions: serde_json::Value,
    ) -> Result<Survey, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, Survey>(
            r#"
            INSERT INTO surveys (id, title, description, questions, active, created_at, updated_at)
            VALUES ($1, $2, $3, $4, true, $5, $6)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(title)
        .bind(description)
        .bind(&questions)
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn send_survey(
        &self,
        survey_id: Uuid,
        customer_id: Uuid,
    ) -> Result<SurveyResponse, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, SurveyResponse>(
            r#"
            INSERT INTO survey_responses (id, survey_id, customer_id, answers, completed_at, created_at)
            VALUES ($1, $2, $3, '{}', NULL, $4)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(survey_id)
        .bind(customer_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn get_responses(
        &self,
        survey_id: Uuid,
    ) -> Result<Vec<SurveyResponse>, sqlx::Error> {
        sqlx::query_as::<_, SurveyResponse>(
            r#"
            SELECT * FROM survey_responses
            WHERE survey_id = $1
            ORDER BY created_at DESC
            "#,
        )
        .bind(survey_id)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_survey(
        &self,
        id: Uuid,
    ) -> Result<Option<Survey>, sqlx::Error> {
        sqlx::query_as::<_, Survey>(
            "SELECT * FROM surveys WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn list_surveys(
        &self,
        experience_id: Option<Uuid>,
        application_id: Option<Uuid>,
        page_id: Option<Uuid>,
        component_id: Option<Uuid>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Survey>, sqlx::Error> {
        sqlx::query_as::<_, Survey>(
            "SELECT * FROM surveys
             WHERE ($1::uuid IS NULL OR experience_id = $1)
               AND ($2::uuid IS NULL OR application_id = $2)
               AND ($3::uuid IS NULL OR page_id = $3)
               AND ($4::uuid IS NULL OR component_id = $4)
             ORDER BY created_at DESC LIMIT $5 OFFSET $6",
        )
        .bind(experience_id)
        .bind(application_id)
        .bind(page_id)
        .bind(component_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count_surveys(
        &self,
        experience_id: Option<Uuid>,
        application_id: Option<Uuid>,
        page_id: Option<Uuid>,
        component_id: Option<Uuid>,
    ) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM surveys
             WHERE ($1::uuid IS NULL OR experience_id = $1)
               AND ($2::uuid IS NULL OR application_id = $2)
               AND ($3::uuid IS NULL OR page_id = $3)
               AND ($4::uuid IS NULL OR component_id = $4)",
        )
        .bind(experience_id)
        .bind(application_id)
        .bind(page_id)
        .bind(component_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn update_survey(
        &self,
        id: Uuid,
        title: Option<&str>,
        description: Option<&str>,
        questions: Option<serde_json::Value>,
        active: Option<bool>,
    ) -> Result<Option<Survey>, sqlx::Error> {
        sqlx::query_as::<_, Survey>(
            r#"
            UPDATE surveys
            SET title = COALESCE($2, title),
                description = COALESCE($3, description),
                questions = COALESCE($4, questions),
                active = COALESCE($5, active),
                updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(title)
        .bind(description)
        .bind(questions)
        .bind(active)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn delete_survey(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM surveys WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn submit_response(
        &self,
        response_id: Uuid,
        answers: serde_json::Value,
    ) -> Result<SurveyResponse, sqlx::Error> {
        sqlx::query_as::<_, SurveyResponse>(
            r#"
            UPDATE survey_responses
            SET answers = $2,
                completed_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(response_id)
        .bind(&answers)
        .fetch_one(&self.pool)
        .await
    }
}
