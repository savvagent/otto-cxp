//! Chat sessions and messages: `ChatExt` on `savvagent_tenant::Tx`,
//! with its model structs.

use savvagent_tenant::Tx;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{map_fk, Error, Result};
use crate::paging::Page;

/// Valid `message_role` values.
pub const ROLES: [&str; 3] = ["customer", "agent", "system"];

const SESSION_COLS: &str = "id, customer_id, status, metadata, started_at, ended_at";
const MSG_COLS: &str = "id, session_id, role::text AS role, content, metadata, created_at";

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: Uuid,
    pub customer_id: Uuid,
    pub status: String,
    pub metadata: serde_json::Value,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: Uuid,
    pub session_id: Uuid,
    pub role: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub trait ChatExt {
    /// Start an `active` session for a customer of this org (id already
    /// resolved). An unknown customer is `NotFound { kind: "customer" }`.
    fn start_chat_session(
        &mut self,
        customer_id: Uuid,
    ) -> impl std::future::Future<Output = Result<ChatSession>> + Send;

    fn get_chat_session(
        &mut self,
        id: Uuid,
    ) -> impl std::future::Future<Output = Result<ChatSession>> + Send;

    /// Append a message, stamped with the wall clock (not the transaction's
    /// start) so messages sent in one transaction keep their order. `role` is
    /// `customer`, `agent`, or `system`. An unknown session is
    /// `NotFound { kind: "chat session" }`.
    fn send_chat_message(
        &mut self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> impl std::future::Future<Output = Result<ChatMessage>> + Send;

    /// A session's transcript, oldest first, with the total message count.
    fn get_chat_messages(
        &mut self,
        session_id: Uuid,
        page: Page,
    ) -> impl std::future::Future<Output = Result<(Vec<ChatMessage>, i64)>> + Send;

    /// Mark the session `ended`; the first end time is kept if it is ended
    /// again.
    fn end_chat_session(
        &mut self,
        id: Uuid,
    ) -> impl std::future::Future<Output = Result<ChatSession>> + Send;
}

/// The row kind a foreign-key violation names, by the exact generated
/// constraint names of `chat_sessions` and `chat_messages` (see [`map_fk`]).
const FK_KINDS: &[(&str, &str)] = &[
    ("chat_sessions_org_id_customer_id_fkey", "customer"),
    ("chat_messages_org_id_session_id_fkey", "chat session"),
];

const NO_SESSION: Error = Error::NotFound {
    kind: "chat session",
};

impl ChatExt for Tx<'_> {
    async fn start_chat_session(&mut self, customer_id: Uuid) -> Result<ChatSession> {
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO chat_sessions (org_id, customer_id) VALUES ($1, $2) \
             RETURNING {SESSION_COLS}"
        ))
        .bind(org)
        .bind(customer_id)
        .fetch_one(self.conn())
        .await
        .map_err(|e| map_fk(e, FK_KINDS))
    }

    async fn get_chat_session(&mut self, id: Uuid) -> Result<ChatSession> {
        let org = self.org();
        sqlx::query_as(&format!(
            "SELECT {SESSION_COLS} FROM chat_sessions WHERE org_id = $1 AND id = $2"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(NO_SESSION)
    }

    async fn send_chat_message(
        &mut self,
        session_id: Uuid,
        role: &str,
        content: &str,
    ) -> Result<ChatMessage> {
        if !ROLES.contains(&role) {
            return Err(Error::Invalid(format!(
                "role must be one of {}",
                ROLES.join(", ")
            )));
        }
        let org = self.org();
        sqlx::query_as(&format!(
            "INSERT INTO chat_messages (org_id, session_id, role, content, created_at) \
             VALUES ($1, $2, $3::message_role, $4, clock_timestamp()) RETURNING {MSG_COLS}"
        ))
        .bind(org)
        .bind(session_id)
        .bind(role)
        .bind(content)
        .fetch_one(self.conn())
        .await
        .map_err(|e| map_fk(e, FK_KINDS))
    }

    async fn get_chat_messages(
        &mut self,
        session_id: Uuid,
        page: Page,
    ) -> Result<(Vec<ChatMessage>, i64)> {
        self.get_chat_session(session_id).await?;
        let org = self.org();
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_messages WHERE org_id = $1 AND session_id = $2",
        )
        .bind(org)
        .bind(session_id)
        .fetch_one(self.conn())
        .await?;
        let rows = sqlx::query_as(&format!(
            "SELECT {MSG_COLS} FROM chat_messages WHERE org_id = $1 AND session_id = $2 \
             ORDER BY created_at ASC, id LIMIT $3 OFFSET $4"
        ))
        .bind(org)
        .bind(session_id)
        .bind(page.limit())
        .bind(page.offset())
        .fetch_all(self.conn())
        .await?;
        Ok((rows, total))
    }

    async fn end_chat_session(&mut self, id: Uuid) -> Result<ChatSession> {
        let org = self.org();
        sqlx::query_as(&format!(
            "UPDATE chat_sessions SET status = 'ended', ended_at = COALESCE(ended_at, clock_timestamp()) \
             WHERE org_id = $1 AND id = $2 RETURNING {SESSION_COLS}"
        ))
        .bind(org)
        .bind(id)
        .fetch_optional(self.conn())
        .await?
        .ok_or(NO_SESSION)
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
