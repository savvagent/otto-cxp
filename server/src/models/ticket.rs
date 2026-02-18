use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Ticket {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub subject: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub channel: Option<String>,
    pub assigned_to: Option<String>,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub resolved_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
