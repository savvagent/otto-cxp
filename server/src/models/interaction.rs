use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Interaction {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub interaction_type: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
}
