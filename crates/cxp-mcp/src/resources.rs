//! The MCP resources: `metrics://dashboard`, and the templates
//! `customer://profiles/{id}` and `knowledge://articles/{id}`. Reading any of
//! them needs read scope (checked by the caller, [`crate::server`]) and runs
//! in a transaction pinned to the caller's org, so another org's id is
//! `not_found`, exactly like a missing one.

use cxp_core::auth::Principal;
use cxp_core::customers::CustomersExt;
use cxp_core::dashboard::DashboardExt;
use cxp_core::knowledge::KnowledgeExt;
use cxp_core::Error;
use rmcp::model::{ErrorData, ReadResourceResult, Resource, ResourceContents, ResourceTemplate};
use serde::Serialize;
use uuid::Uuid;

use crate::server::{Cxp, McpResult};

pub const DASHBOARD: &str = "metrics://dashboard";
pub const CUSTOMER_PREFIX: &str = "customer://profiles/";
pub const ARTICLE_PREFIX: &str = "knowledge://articles/";

const JSON: &str = "application/json";

/// The concrete resources.
pub fn list() -> Vec<Resource> {
    vec![Resource::new(DASHBOARD, "Metrics Dashboard")
        .with_title("Metrics Dashboard")
        .with_description(
            "Ticket counts by status, CSAT, and the mean feedback rating for this organization",
        )
        .with_mime_type(JSON)]
}

/// The parameterized resources.
pub fn templates() -> Vec<ResourceTemplate> {
    vec![
        ResourceTemplate::new(format!("{CUSTOMER_PREFIX}{{id}}"), "Customer Profile")
            .with_title("Customer Profile")
            .with_description("A customer profile by id")
            .with_mime_type(JSON),
        ResourceTemplate::new(format!("{ARTICLE_PREFIX}{{id}}"), "Knowledge Article")
            .with_title("Knowledge Article")
            .with_description("A knowledge base article by id")
            .with_mime_type(JSON),
    ]
}

/// What a resource URI names.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Dashboard,
    Customer(Uuid),
    Article(Uuid),
}

fn parse(uri: &str) -> Result<Target, Error> {
    let id = |s: &str| {
        Uuid::parse_str(s).map_err(|_| Error::Invalid(format!("{s:?} in {uri} is not a uuid")))
    };
    if uri == DASHBOARD {
        Ok(Target::Dashboard)
    } else if let Some(rest) = uri.strip_prefix(CUSTOMER_PREFIX) {
        Ok(Target::Customer(id(rest)?))
    } else if let Some(rest) = uri.strip_prefix(ARTICLE_PREFIX) {
        Ok(Target::Article(id(rest)?))
    } else {
        Err(Error::Invalid(format!(
            "unknown resource {uri}; this server serves {DASHBOARD}, {CUSTOMER_PREFIX}{{id}}, \
             and {ARTICLE_PREFIX}{{id}}"
        )))
    }
}

fn contents(uri: &str, value: &impl Serialize) -> Result<ReadResourceResult, ErrorData> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|_| crate::error::internal("could not encode a resource"))?;
    Ok(ReadResourceResult::new(vec![ResourceContents::text(
        text, uri,
    )
    .with_mime_type(JSON)]))
}

/// Read one resource. A missing row (or another org's) is MCP's
/// resource-not-found, with `data.code` `not_found`.
pub(crate) async fn read(
    cxp: &Cxp,
    caller: &Principal,
    uri: &str,
) -> Result<ReadResourceResult, ErrorData> {
    read_target(cxp, caller, uri)
        .await
        .map_err(crate::error::as_resource_not_found)
}

async fn read_target(
    cxp: &Cxp,
    caller: &Principal,
    uri: &str,
) -> Result<ReadResourceResult, ErrorData> {
    let target = parse(uri).mcp()?;
    let mut tx = cxp.tx(caller).await?;
    let out = match target {
        Target::Dashboard => contents(uri, &tx.dashboard_summary().await.mcp()?),
        Target::Customer(id) => contents(uri, &tx.get_customer(id).await.mcp()?),
        Target::Article(id) => contents(uri, &tx.get_article(id).await.mcp()?),
    };
    tx.commit().await.mcp()?;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_parse_to_what_they_name() {
        let id = Uuid::new_v4();
        assert_eq!(parse(DASHBOARD).unwrap(), Target::Dashboard);
        assert_eq!(
            parse(&format!("customer://profiles/{id}")).unwrap(),
            Target::Customer(id)
        );
        assert_eq!(
            parse(&format!("knowledge://articles/{id}")).unwrap(),
            Target::Article(id)
        );
        for bad in [
            "customer://profiles/nope",
            "knowledge://articles/",
            "file:///etc/passwd",
        ] {
            assert_eq!(parse(bad).unwrap_err().code(), "invalid_argument", "{bad}");
        }
    }

    #[test]
    fn three_resources_are_advertised() {
        let mut uris: Vec<String> = list().into_iter().map(|r| r.uri).collect();
        uris.extend(templates().into_iter().map(|t| t.uri_template));
        assert_eq!(
            uris,
            [
                "metrics://dashboard",
                "customer://profiles/{id}",
                "knowledge://articles/{id}"
            ]
        );
    }
}
