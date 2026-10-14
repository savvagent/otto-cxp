//! Support: tickets, the knowledge base, and chat.

use cxp_core::chat::{ChatExt, ChatMessage, ChatSession};
use cxp_core::customers::CustomersExt;
use cxp_core::knowledge::{KnowledgeArticle, KnowledgeExt};
use cxp_core::tickets::{NewTicket, Ticket, TicketFilter, TicketUpdate, TicketsExt};
use cxp_core::Page;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::{tool, tool_router};
use serde::Deserialize;

use super::out::List;
use super::{uuid, IdParam, QueryParam};
use crate::server::{Cxp, McpResult};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTicketParams {
    /// The customer: a UUID, an exact email, or a name.
    pub customer_id: String,
    pub subject: String,
    #[serde(default)]
    pub description: Option<String>,
    /// low, medium (the default), high, or urgent.
    #[serde(default)]
    pub priority: Option<String>,
    /// email, chat, phone, web, or social.
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateTicketParams {
    /// The ticket's UUID.
    pub id: String,
    /// open, in_progress, waiting, resolved, or closed.
    #[serde(default)]
    pub status: Option<String>,
    /// low, medium, high, or urgent.
    #[serde(default)]
    pub priority: Option<String>,
    /// Free text naming who owns the ticket, e.g. an agent's name or email.
    #[serde(default)]
    pub assigned_to: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListTicketsParams {
    /// Only this customer's: a UUID, an exact email, or a name.
    #[serde(default)]
    pub customer_id: Option<String>,
    /// Only tickets in this status: open, in_progress, waiting, resolved, or
    /// closed.
    #[serde(default)]
    pub status: Option<String>,
    /// Only tickets of this priority: low, medium, high, or urgent.
    #[serde(default)]
    pub priority: Option<String>,
    /// At most this many (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Skip this many first.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChatMessageParams {
    /// The chat session's UUID.
    pub session_id: String,
    /// customer, agent, or system.
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SessionIdParam {
    /// The chat session's UUID.
    pub session_id: String,
    /// At most this many (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Skip this many first.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[tool_router(router = support_router, vis = "pub(crate)")]
impl Cxp {
    #[tool(
        name = "create_ticket",
        description = "Create a new support ticket. Accepts UUID, email, or name for customer_id. \
                       Needs cxp:write.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn create_ticket(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<CreateTicketParams>,
    ) -> Result<Json<Ticket>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let customer_id = tx.resolve_customer(&p.customer_id).await.mcp()?;
        let ticket = tx
            .create_ticket(&NewTicket {
                customer_id,
                subject: p.subject,
                description: p.description,
                priority: p.priority,
                channel: p.channel,
            })
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(ticket))
    }

    #[tool(
        name = "update_ticket",
        description = "Update a support ticket's status (open, in_progress, waiting, resolved, \
                       closed), priority (low, medium, high, urgent), or assigned_to (free text, \
                       e.g. an agent's name or email). Needs cxp:write.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    pub async fn update_ticket(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<UpdateTicketParams>,
    ) -> Result<Json<Ticket>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let ticket = tx
            .update_ticket(
                id,
                &TicketUpdate {
                    status: p.status,
                    priority: p.priority,
                    assigned_to: p.assigned_to,
                    ..TicketUpdate::default()
                },
            )
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(ticket))
    }

    #[tool(
        name = "get_ticket",
        description = "Get a support ticket by ID.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_ticket(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<IdParam>,
    ) -> Result<Json<Ticket>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let ticket = tx.get_ticket(id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(ticket))
    }

    #[tool(
        name = "list_tickets",
        description = "List support tickets, newest first, with optional filters: customer_id \
                       (UUID, email, or name), status (open, in_progress, waiting, resolved, \
                       closed), priority (low, medium, high, urgent). Returns {items, total}.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_tickets(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<ListTicketsParams>,
    ) -> Result<Json<List<Ticket>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let customer_id = match p.customer_id.as_deref() {
            Some(c) => Some(tx.resolve_customer(c).await.mcp()?),
            None => None,
        };
        let page = tx
            .list_tickets(
                &TicketFilter {
                    customer_id,
                    status: p.status,
                    priority: p.priority,
                    ..TicketFilter::default()
                },
                Page::new(p.limit, p.offset),
            )
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::counted(page)))
    }

    #[tool(
        name = "close_ticket",
        description = "Close a support ticket and set its resolved timestamp. Needs cxp:write.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    pub async fn close_ticket(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<IdParam>,
    ) -> Result<Json<Ticket>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let ticket = tx.close_ticket(id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(ticket))
    }

    #[tool(
        name = "search_knowledge_base",
        description = "Search the published knowledge base articles; best matches first, at most \
                       20. Returns {items}.",
        annotations(read_only_hint = true)
    )]
    pub async fn search_knowledge_base(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<QueryParam>,
    ) -> Result<Json<List<KnowledgeArticle>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let found = tx.search_articles(&p.query).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::uncounted(found)))
    }

    #[tool(
        name = "start_chat_session",
        description = "Start a new chat session for a customer. `id` is the customer: a UUID, \
                       an exact email, or a name. Needs cxp:write.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn start_chat_session(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<IdParam>,
    ) -> Result<Json<ChatSession>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let customer_id = tx.resolve_customer(&p.id).await.mcp()?;
        let session = tx.start_chat_session(customer_id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(session))
    }

    #[tool(
        name = "send_chat_message",
        description = "Send a message in a chat session. Needs cxp:write.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn send_chat_message(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<ChatMessageParams>,
    ) -> Result<Json<ChatMessage>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let session_id = uuid("session_id", &p.session_id)?;
        let mut tx = self.tx(&caller).await?;
        let message = tx
            .send_chat_message(session_id, &p.role, &p.content)
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(message))
    }

    #[tool(
        name = "get_chat_messages",
        description = "Get the messages in a chat session, oldest first. Returns {items, total}.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_chat_messages(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<SessionIdParam>,
    ) -> Result<Json<List<ChatMessage>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let session_id = uuid("session_id", &p.session_id)?;
        let mut tx = self.tx(&caller).await?;
        let page = tx
            .get_chat_messages(session_id, Page::new(p.limit, p.offset))
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::counted(page)))
    }

    #[tool(
        name = "end_chat_session",
        description = "End an active chat session. Needs cxp:write.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    pub async fn end_chat_session(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<IdParam>,
    ) -> Result<Json<ChatSession>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let session = tx.end_chat_session(id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(session))
    }
}
