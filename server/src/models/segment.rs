use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Segment {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub criteria: serde_json::Value,
    pub segment_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
