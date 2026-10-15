//! Customer data: profiles, interactions, segments, and the 360 view.

use cxp_core::customers::{
    Customer, Customer360, CustomerFilter, CustomerUpdate, CustomersExt, NewCustomer,
};
use cxp_core::interactions::{parse_json, Interaction, InteractionsExt, NewInteraction};
use cxp_core::segments::{NewSegment, Segment, SegmentsExt};
use cxp_core::Page;
use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::{tool, tool_router};
use serde::Deserialize;

use super::out::List;
use super::{uuid, CustomerIdParam, QueryParam};
use crate::server::{Cxp, McpResult};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateCustomerParams {
    /// The customer's email, unique in this organization.
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateCustomerParams {
    /// The customer: a UUID, an exact email, or a name
    /// (matched exactly, case-insensitively, else by a unique partial match of
    /// the name or email; several matches are refused).
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListCustomersParams {
    /// At most this many (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Skip this many first.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecordInteractionParams {
    /// The customer: a UUID, an exact email, or a name
    /// (matched exactly, case-insensitively, else by a unique partial match of
    /// the name or email; several matches are refused).
    pub customer_id: String,
    /// One of page_view, purchase, support_contact, email_open, email_click,
    /// feature_use, login.
    pub interaction_type: String,
    /// Optional JSON object, as text.
    #[serde(default)]
    pub metadata: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetJourneyParams {
    /// The customer: a UUID, an exact email, or a name
    /// (matched exactly, case-insensitively, else by a unique partial match of
    /// the name or email; several matches are refused).
    pub customer_id: String,
    /// At most this many (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Skip this many first.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateSegmentParams {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// A JSON object, as text. A dynamic segment supports tags_contain,
    /// company_eq, created_after, and created_before.
    pub criteria: String,
    /// "dynamic" (the default) or "static".
    #[serde(default)]
    pub segment_type: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SegmentMembersParams {
    /// The segment's UUID.
    pub id: String,
    /// At most this many (1 to 200, default 50).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Skip this many first.
    #[serde(default)]
    pub offset: Option<i64>,
}

#[tool_router(router = cdp_router, vis = "pub(crate)")]
impl Cxp {
    #[tool(
        name = "create_customer",
        description = "Create a new customer profile. Needs cxp:write.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn create_customer(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<CreateCustomerParams>,
    ) -> Result<Json<Customer>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let customer = tx
            .create_customer(&NewCustomer {
                email: p.email,
                name: p.name,
                phone: p.phone,
                company: p.company,
                metadata: None,
                tags: vec![],
            })
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(customer))
    }

    #[tool(
        name = "list_customers",
        description = "List customers, newest first, with pagination. Returns {items, total}.",
        annotations(read_only_hint = true)
    )]
    pub async fn list_customers(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<ListCustomersParams>,
    ) -> Result<Json<List<Customer>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let page = tx
            .list_customers(&CustomerFilter::default(), Page::new(p.limit, p.offset))
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::counted(page)))
    }

    #[tool(
        name = "update_customer",
        description = "Update an existing customer profile. Accepts UUID, email, or name for the \
                       id field. Needs cxp:write.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true
        )
    )]
    pub async fn update_customer(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<UpdateCustomerParams>,
    ) -> Result<Json<Customer>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let id = tx.resolve_customer(&p.id).await.mcp()?;
        let customer = tx
            .update_customer(
                id,
                &CustomerUpdate {
                    email: p.email,
                    name: p.name,
                    phone: p.phone,
                    company: p.company,
                    metadata: None,
                    tags: None,
                },
            )
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(customer))
    }

    #[tool(
        name = "get_customer",
        description = "Get a customer by ID. Accepts UUID, email, or name.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_customer(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<CustomerIdParam>,
    ) -> Result<Json<Customer>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let id = tx.resolve_customer(&p.id).await.mcp()?;
        let customer = tx.get_customer(id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(customer))
    }

    #[tool(
        name = "search_customers",
        description = "Search customers by name or email; best matches first, at most 50. Returns \
                       {items, total, totalCapped}. totalCapped false: total is the exact count \
                       and items are exactly the best matches. totalCapped true: total is a \
                       lower bound (1000 when more than 1000 match, or fewer when the search \
                       stopped before counting every match), and items are the best-ranked of \
                       the matches found: narrow the query to reach others. A search too \
                       broad to finish in time fails with code query_too_broad: narrow it.",
        annotations(read_only_hint = true)
    )]
    pub async fn search_customers(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<QueryParam>,
    ) -> Result<Json<List<Customer>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let found = tx.search_customers(&p.query, Page::default()).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::searched(found)))
    }

    #[tool(
        name = "record_interaction",
        description = "Record a customer interaction event. Accepts UUID, email, or name for \
                       customer_id. Needs cxp:write.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn record_interaction(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<RecordInteractionParams>,
    ) -> Result<Json<Interaction>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let metadata = p
            .metadata
            .as_deref()
            .map(|m| parse_json("metadata", m))
            .transpose()
            .mcp()?;
        let mut tx = self.tx(&caller).await?;
        let customer_id = tx.resolve_customer(&p.customer_id).await.mcp()?;
        let interaction = tx
            .record_interaction(&NewInteraction {
                customer_id,
                interaction_type: p.interaction_type,
                metadata,
            })
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(interaction))
    }

    #[tool(
        name = "get_journey",
        description = "Get the interaction journey for a customer, newest first. Accepts UUID, \
                       email, or name for customer_id. Returns {items, total}.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_journey(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<GetJourneyParams>,
    ) -> Result<Json<List<Interaction>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let customer_id = tx.resolve_customer(&p.customer_id).await.mcp()?;
        let page = tx
            .get_journey(customer_id, Page::new(p.limit, p.offset))
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::counted(page)))
    }

    #[tool(
        name = "create_segment",
        description = "Create a customer segment with filter criteria (a JSON object, as text). \
                       Needs cxp:write.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    pub async fn create_segment(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<CreateSegmentParams>,
    ) -> Result<Json<Segment>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_write().mcp()?;
        let criteria = parse_json("criteria", &p.criteria).mcp()?;
        let mut tx = self.tx(&caller).await?;
        let segment = tx
            .create_segment(&NewSegment {
                name: p.name,
                description: p.description,
                criteria,
                segment_type: p.segment_type,
            })
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(segment))
    }

    #[tool(
        name = "get_segment_members",
        description = "Get members of a customer segment, newest first. Returns {items, total}.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_segment_members(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<SegmentMembersParams>,
    ) -> Result<Json<List<Customer>>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let id = uuid("id", &p.id)?;
        let mut tx = self.tx(&caller).await?;
        let page = tx
            .get_segment_members(id, Page::new(p.limit, p.offset))
            .await
            .mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(List::counted(page)))
    }

    #[tool(
        name = "get_customer_360",
        description = "Get a full 360-degree view of a customer including their recent tickets, \
                       interactions, and feedback. Accepts UUID, email, or name.",
        annotations(read_only_hint = true)
    )]
    pub async fn get_customer_360(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(p): Parameters<CustomerIdParam>,
    ) -> Result<Json<Customer360>, ErrorData> {
        let caller = self.caller(&parts)?;
        caller.require_read().mcp()?;
        let mut tx = self.tx(&caller).await?;
        let id = tx.resolve_customer(&p.id).await.mcp()?;
        let view = tx.customer_360(id).await.mcp()?;
        tx.commit().await.mcp()?;
        Ok(Json(view))
    }
}
