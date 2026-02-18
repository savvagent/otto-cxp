use std::future::Future;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams,
    ReadResourceResult, ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db;

mod cdp;
mod feedback;
mod support;

fn parse_uuid(s: &str) -> Result<Uuid, String> {
    Uuid::parse_str(s).map_err(|e| format!("Invalid UUID: {e}"))
}

// Parameter structs for tools
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CreateCustomerParams {
    email: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    company: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct UpdateCustomerParams {
    id: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    phone: Option<String>,
    #[serde(default)]
    company: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct IdParam {
    id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct QueryParam {
    query: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RecordInteractionParams {
    customer_id: String,
    interaction_type: String,
    #[serde(default)]
    metadata: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetJourneyParams {
    customer_id: String,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CreateSegmentParams {
    name: String,
    #[serde(default)]
    description: Option<String>,
    criteria: String,
    #[serde(default)]
    segment_type: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CreateTicketParams {
    customer_id: String,
    subject: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    channel: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct UpdateTicketParams {
    id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    assigned_to: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListTicketsParams {
    #[serde(default)]
    customer_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CreateSurveyParams {
    title: String,
    #[serde(default)]
    description: Option<String>,
    questions: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SendSurveyParams {
    survey_id: String,
    customer_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SubmitSurveyResponseParams {
    survey_id: String,
    customer_id: String,
    answers: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SubmitFeedbackParams {
    customer_id: String,
    feedback_type: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    rating: Option<i32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GetFeedbackParams {
    #[serde(default)]
    customer_id: Option<String>,
    #[serde(default)]
    feedback_type: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CalculateCsatParams {
    #[serde(default)]
    feedback_type: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct TextParam {
    text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ListCustomersParams {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ChatMessageParams {
    session_id: String,
    role: String,
    content: String,
}

#[derive(Clone)]
pub struct CxpServer {
    pub customers: db::customers::CustomerDb,
    pub tickets: db::tickets::TicketDb,
    pub feedback: db::feedback::FeedbackDb,
    pub knowledge: db::knowledge::KnowledgeDb,
    pub interactions: db::interactions::InteractionDb,
    pub segments: db::segments::SegmentDb,
    pub chat: db::chat::ChatDb,
    tool_router: ToolRouter<Self>,
}

impl CxpServer {
    pub fn new(pool: PgPool) -> Self {
        Self {
            customers: db::customers::CustomerDb::new(pool.clone()),
            tickets: db::tickets::TicketDb::new(pool.clone()),
            feedback: db::feedback::FeedbackDb::new(pool.clone()),
            knowledge: db::knowledge::KnowledgeDb::new(pool.clone()),
            interactions: db::interactions::InteractionDb::new(pool.clone()),
            segments: db::segments::SegmentDb::new(pool.clone()),
            chat: db::chat::ChatDb::new(pool),
            tool_router: Self::tool_router(),
        }
    }
}

impl CxpServer {
    /// Resolve a customer identifier (UUID, email, or name) to a UUID.
    async fn resolve_customer(&self, identifier: &str) -> Result<Uuid, String> {
        // Try UUID first
        if let Ok(uuid) = Uuid::parse_str(identifier) {
            return Ok(uuid);
        }
        // Try email lookup
        if identifier.contains('@') {
            return match self.customers.get_by_email(identifier).await {
                Ok(Some(c)) => Ok(c.id),
                Ok(None) => Err(format!("No customer found with email: {identifier}")),
                Err(e) => Err(format!("Error looking up email: {e}")),
            };
        }
        // Fall back to name search
        match self.customers.search(identifier).await {
            Ok(results) if results.is_empty() => {
                Err(format!("No customer found matching: {identifier}"))
            }
            Ok(results) if results.len() == 1 => Ok(results[0].id),
            Ok(results) => {
                let names: Vec<String> = results
                    .iter()
                    .take(5)
                    .map(|c| format!("{} ({})", c.name.as_deref().unwrap_or("unnamed"), c.email))
                    .collect();
                Err(format!(
                    "Multiple customers match '{}'. Please be more specific: {}",
                    identifier,
                    names.join(", ")
                ))
            }
            Err(e) => Err(format!("Error searching customers: {e}")),
        }
    }
}

#[tool_router]
impl CxpServer {
    // ─── CDP Tools ───────────────────────────────────────────────────────

    #[tool(description = "Create a new customer profile")]
    async fn create_customer(
        &self,
        Parameters(params): Parameters<CreateCustomerParams>,
    ) -> String {
        match self
            .customers
            .create(
                &params.email,
                params.name.as_deref(),
                params.phone.as_deref(),
                params.company.as_deref(),
                serde_json::Value::Object(Default::default()),
                &[],
            )
            .await
        {
            Ok(c) => serde_json::to_string_pretty(&c).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "List customers with optional pagination")]
    async fn list_customers(
        &self,
        Parameters(params): Parameters<ListCustomersParams>,
    ) -> String {
        let limit = params.limit.unwrap_or(50);
        let offset = params.offset.unwrap_or(0);
        match self.customers.list(limit, offset).await {
            Ok(customers) => serde_json::to_string_pretty(&customers).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Update an existing customer profile. Accepts UUID, email, or name for the id field")]
    async fn update_customer(
        &self,
        Parameters(params): Parameters<UpdateCustomerParams>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self
            .customers
            .update(
                uuid,
                params.email.as_deref(),
                params.name.as_deref(),
                params.phone.as_deref(),
                params.company.as_deref(),
                None,
                None,
            )
            .await
        {
            Ok(Some(c)) => serde_json::to_string_pretty(&c).unwrap(),
            Ok(None) => format!("Customer not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get a customer by ID. Accepts UUID, email, or name")]
    async fn get_customer(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.customers.get(uuid).await {
            Ok(Some(c)) => serde_json::to_string_pretty(&c).unwrap(),
            Ok(None) => format!("Customer not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Search customers by name or email")]
    async fn search_customers(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> String {
        match self.customers.search(&params.query).await {
            Ok(results) => serde_json::to_string_pretty(&results).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Record a customer interaction event. Accepts UUID, email, or name for customer_id")]
    async fn record_interaction(
        &self,
        Parameters(params): Parameters<RecordInteractionParams>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.customer_id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        let meta = match params.metadata {
            Some(ref s) => match serde_json::from_str(s) {
                Ok(v) => v,
                Err(e) => return format!("Invalid metadata JSON: {e}"),
            },
            None => serde_json::Value::Object(Default::default()),
        };
        match self.interactions.record(uuid, &params.interaction_type, meta).await {
            Ok(i) => serde_json::to_string_pretty(&i).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get the interaction journey for a customer. Accepts UUID, email, or name for customer_id")]
    async fn get_journey(
        &self,
        Parameters(params): Parameters<GetJourneyParams>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.customer_id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        let limit = params.limit.unwrap_or(50);
        match self.interactions.get_journey(uuid, limit).await {
            Ok(interactions) => serde_json::to_string_pretty(&interactions).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Create a customer segment with filter criteria")]
    async fn create_segment(
        &self,
        Parameters(params): Parameters<CreateSegmentParams>,
    ) -> String {
        let criteria_json: serde_json::Value = match serde_json::from_str(&params.criteria) {
            Ok(v) => v,
            Err(e) => return format!("Invalid criteria JSON: {e}"),
        };
        let seg_type = params.segment_type.as_deref().unwrap_or("dynamic");
        match self
            .segments
            .create(&params.name, params.description.as_deref(), criteria_json, seg_type)
            .await
        {
            Ok(s) => serde_json::to_string_pretty(&s).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get members of a customer segment")]
    async fn get_segment_members(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.segments.get_members(uuid).await {
            Ok(members) => serde_json::to_string_pretty(&members).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get a full 360-degree view of a customer including tickets, interactions, and feedback. Accepts UUID, email, or name")]
    async fn get_customer_360(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.customers.get_360(uuid).await {
            Ok(Some(view)) => serde_json::to_string_pretty(&view).unwrap(),
            Ok(None) => format!("Customer not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    // ─── Support Tools ───────────────────────────────────────────────────

    #[tool(description = "Create a new support ticket. Accepts UUID, email, or name for customer_id")]
    async fn create_ticket(
        &self,
        Parameters(params): Parameters<CreateTicketParams>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.customer_id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        let priority = params.priority.as_deref().unwrap_or("medium");
        match self
            .tickets
            .create(uuid, &params.subject, params.description.as_deref(), priority, params.channel.as_deref())
            .await
        {
            Ok(t) => serde_json::to_string_pretty(&t).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Update a support ticket's status, priority, or assignment")]
    async fn update_ticket(
        &self,
        Parameters(params): Parameters<UpdateTicketParams>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self
            .tickets
            .update(uuid, params.status.as_deref(), params.priority.as_deref(), params.assigned_to.as_deref(), None)
            .await
        {
            Ok(Some(t)) => serde_json::to_string_pretty(&t).unwrap(),
            Ok(None) => format!("Ticket not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get a support ticket by ID")]
    async fn get_ticket(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.tickets.get(uuid).await {
            Ok(Some(t)) => serde_json::to_string_pretty(&t).unwrap(),
            Ok(None) => format!("Ticket not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "List support tickets with optional filters. Accepts UUID, email, or name for customer_id")]
    async fn list_tickets(
        &self,
        Parameters(params): Parameters<ListTicketsParams>,
    ) -> String {
        let cid = match params.customer_id {
            Some(ref s) => match self.resolve_customer(s).await {
                Ok(u) => Some(u),
                Err(e) => return e,
            },
            None => None,
        };
        let limit = params.limit.unwrap_or(50);
        let offset = params.offset.unwrap_or(0);
        match self
            .tickets
            .list(cid, params.status.as_deref(), params.priority.as_deref(), limit, offset)
            .await
        {
            Ok(tickets) => serde_json::to_string_pretty(&tickets).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Close a support ticket and set its resolved timestamp")]
    async fn close_ticket(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.tickets.close(uuid).await {
            Ok(Some(t)) => serde_json::to_string_pretty(&t).unwrap(),
            Ok(None) => format!("Ticket not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Search the knowledge base for relevant articles")]
    async fn search_knowledge_base(
        &self,
        Parameters(params): Parameters<QueryParam>,
    ) -> String {
        match self.knowledge.search(&params.query).await {
            Ok(articles) => serde_json::to_string_pretty(&articles).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Start a new chat session for a customer. Accepts UUID, email, or name")]
    async fn start_chat_session(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.chat.start_session(uuid).await {
            Ok(s) => serde_json::to_string_pretty(&s).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Send a message in a chat session")]
    async fn send_chat_message(
        &self,
        Parameters(params): Parameters<ChatMessageParams>,
    ) -> String {
        let uuid = match parse_uuid(&params.session_id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.chat.send_message(uuid, &params.role, &params.content).await {
            Ok(m) => serde_json::to_string_pretty(&m).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "End an active chat session")]
    async fn end_chat_session(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.chat.end_session(uuid).await {
            Ok(Some(s)) => serde_json::to_string_pretty(&s).unwrap(),
            Ok(None) => format!("Chat session not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    // ─── Feedback Tools ──────────────────────────────────────────────────

    #[tool(description = "Get a survey by ID")]
    async fn get_survey(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.feedback.get_survey(uuid).await {
            Ok(Some(s)) => serde_json::to_string_pretty(&s).unwrap(),
            Ok(None) => format!("Survey not found: {}", params.id),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Submit a survey response with answers for a customer. Accepts UUID, email, or name for customer_id")]
    async fn submit_survey_response(
        &self,
        Parameters(params): Parameters<SubmitSurveyResponseParams>,
    ) -> String {
        let survey_id = match parse_uuid(&params.survey_id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        let customer_id = match self.resolve_customer(&params.customer_id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        let answers_json: serde_json::Value = match serde_json::from_str(&params.answers) {
            Ok(v) => v,
            Err(e) => return format!("Invalid answers JSON: {e}"),
        };
        // Create the response record
        let response = match self.feedback.send_survey(survey_id, customer_id).await {
            Ok(r) => r,
            Err(e) => return format!("Error creating response: {e}"),
        };
        // Submit the answers
        match self.feedback.submit_response(response.id, answers_json).await {
            Ok(r) => serde_json::to_string_pretty(&r).unwrap(),
            Err(e) => format!("Error submitting answers: {e}"),
        }
    }

    #[tool(description = "Create a new survey with questions")]
    async fn create_survey(
        &self,
        Parameters(params): Parameters<CreateSurveyParams>,
    ) -> String {
        let questions_json: serde_json::Value = match serde_json::from_str(&params.questions) {
            Ok(v) => v,
            Err(e) => return format!("Invalid questions JSON: {e}"),
        };
        match self
            .feedback
            .create_survey(&params.title, params.description.as_deref(), questions_json)
            .await
        {
            Ok(s) => serde_json::to_string_pretty(&s).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Send a survey to a customer. Accepts UUID, email, or name for customer_id")]
    async fn send_survey(
        &self,
        Parameters(params): Parameters<SendSurveyParams>,
    ) -> String {
        let sid = match parse_uuid(&params.survey_id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        let cid = match self.resolve_customer(&params.customer_id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.feedback.send_survey(sid, cid).await {
            Ok(r) => serde_json::to_string_pretty(&r).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get all responses for a survey")]
    async fn get_survey_responses(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self.feedback.get_responses(uuid).await {
            Ok(responses) => serde_json::to_string_pretty(&responses).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Submit customer feedback. Accepts UUID, email, or name for customer_id")]
    async fn submit_feedback(
        &self,
        Parameters(params): Parameters<SubmitFeedbackParams>,
    ) -> String {
        let uuid = match self.resolve_customer(&params.customer_id).await {
            Ok(u) => u,
            Err(e) => return e,
        };
        match self
            .feedback
            .submit_feedback(uuid, &params.feedback_type, params.subject.as_deref(), params.body.as_deref(), params.rating)
            .await
        {
            Ok(f) => serde_json::to_string_pretty(&f).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get feedback entries with optional filters")]
    async fn get_feedback(
        &self,
        Parameters(params): Parameters<GetFeedbackParams>,
    ) -> String {
        let cid = match params.customer_id {
            Some(ref s) => match parse_uuid(s) {
                Ok(u) => Some(u),
                Err(e) => return e,
            },
            None => None,
        };
        let limit = params.limit.unwrap_or(50);
        match self
            .feedback
            .get_feedback(cid, params.feedback_type.as_deref(), limit)
            .await
        {
            Ok(entries) => serde_json::to_string_pretty(&entries).unwrap(),
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Calculate Net Promoter Score for a survey")]
    async fn calculate_nps(
        &self,
        Parameters(params): Parameters<IdParam>,
    ) -> String {
        let uuid = match parse_uuid(&params.id) {
            Ok(u) => u,
            Err(e) => return e,
        };
        let responses = match self.feedback.get_responses(uuid).await {
            Ok(r) => r,
            Err(e) => return format!("Error fetching responses: {e}"),
        };
        let scores: Vec<i32> = responses
            .iter()
            .filter_map(|r| {
                r.answers
                    .get("score")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32)
            })
            .collect();
        match crate::analysis::calculate_nps(&scores) {
            Some(nps) => {
                json!({
                    "nps": nps,
                    "total_responses": responses.len(),
                    "scored_responses": scores.len(),
                    "promoters": scores.iter().filter(|&&s| s >= 9).count(),
                    "passives": scores.iter().filter(|&&s| s >= 7 && s <= 8).count(),
                    "detractors": scores.iter().filter(|&&s| s <= 6).count(),
                })
                .to_string()
            }
            None => json!({"error": "No scores available to calculate NPS"}).to_string(),
        }
    }

    #[tool(description = "Calculate Customer Satisfaction Score")]
    async fn calculate_csat(
        &self,
        Parameters(params): Parameters<CalculateCsatParams>,
    ) -> String {
        let entries = match self
            .feedback
            .get_feedback(None, params.feedback_type.as_deref(), 10000)
            .await
        {
            Ok(e) => e,
            Err(e) => return format!("Error fetching feedback: {e}"),
        };
        let ratings: Vec<i32> = entries.iter().filter_map(|f| f.rating).collect();
        match crate::analysis::calculate_csat(&ratings) {
            Some(csat) => {
                json!({
                    "csat": csat,
                    "total_feedback": entries.len(),
                    "rated_feedback": ratings.len(),
                    "satisfied": ratings.iter().filter(|&&r| r >= 4).count(),
                    "unsatisfied": ratings.iter().filter(|&&r| r < 4).count(),
                })
                .to_string()
            }
            None => json!({"error": "No ratings available to calculate CSAT"}).to_string(),
        }
    }

    #[tool(description = "Analyze the sentiment of a text string")]
    async fn analyze_sentiment(
        &self,
        Parameters(params): Parameters<TextParam>,
    ) -> String {
        let result = crate::analysis::analyze_sentiment(&params.text);
        serde_json::to_string_pretty(&result).unwrap()
    }
}

#[tool_handler]
impl ServerHandler for CxpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("A comprehensive Customer Experience Platform (CXP) MCP server providing CDP, support ticketing, feedback management, and analytics capabilities.".into()),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
            ..Default::default()
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, rmcp::model::ErrorData>> + Send + '_ {
        let resources = CxpServer::list_resources();
        std::future::ready(Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        }))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, rmcp::model::ErrorData>> + Send + '_ {
        async move {
            let uri = &request.uri;
            match self.handle_resource_read(uri).await {
                Ok(text) => Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(text, uri.clone())],
                }),
                Err(e) => Err(rmcp::model::ErrorData::internal_error(e, None)),
            }
        }
    }
}
