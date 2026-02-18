use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackEmbedding {
    pub id: Uuid,
    pub feedback_id: Uuid,
    pub chunk_index: i32,
    pub chunk_text: String,
    #[sqlx(skip)]
    pub embedding: Option<Vec<f32>>,
    pub created_at: DateTime<Utc>,
}
