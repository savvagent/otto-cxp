use sqlx::PgPool;
use uuid::Uuid;

use crate::models::chat::{ChatMessage, ChatSession};

const MSG_COLS: &str = r#"
    id, session_id, role::text as role, content, metadata, created_at
"#;

#[derive(Debug, Clone)]
pub struct ChatDb {
    pool: PgPool,
}

impl ChatDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn start_session(
        &self,
        customer_id: Uuid,
    ) -> Result<ChatSession, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        sqlx::query_as::<_, ChatSession>(
            r#"
            INSERT INTO chat_sessions (id, customer_id, status, metadata, started_at, ended_at)
            VALUES ($1, $2, 'active', '{}', $3, NULL)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(customer_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn send_message(
        &self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<ChatMessage, sqlx::Error> {
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        let sql = format!(
            "INSERT INTO chat_messages (id, session_id, role, content, metadata, created_at)
             VALUES ($1, $2, $3::message_role, $4, '{{}}', $5)
             RETURNING {MSG_COLS}"
        );

        sqlx::query_as::<_, ChatMessage>(&sql)
            .bind(id)
            .bind(session_id)
            .bind(role)
            .bind(content)
            .bind(now)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn get_messages(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<ChatMessage>, sqlx::Error> {
        let sql = format!(
            "SELECT {MSG_COLS} FROM chat_messages
             WHERE session_id = $1
             ORDER BY created_at ASC"
        );

        sqlx::query_as::<_, ChatMessage>(&sql)
            .bind(session_id)
            .fetch_all(&self.pool)
            .await
    }

    pub async fn end_session(
        &self,
        session_id: Uuid,
    ) -> Result<Option<ChatSession>, sqlx::Error> {
        sqlx::query_as::<_, ChatSession>(
            r#"
            UPDATE chat_sessions
            SET status = 'ended',
                ended_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
    }
}
