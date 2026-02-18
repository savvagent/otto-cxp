use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
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

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SurveyResponse {
    pub id: Uuid,
    pub survey_id: Uuid,
    pub customer_id: Uuid,
    pub answers: serde_json::Value,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
