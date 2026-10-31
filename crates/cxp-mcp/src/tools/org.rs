//! Who am I, and where this service's meters and budget stand. Both check no scope and record no
//! usage: a caller must never have to spend an operation, or hold a
//! particular grant, to find out where it stands.

use rmcp::handler::server::tool::Extension;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::ErrorData;
use rmcp::{tool, tool_router};
use serde::Deserialize;

use super::out;
use crate::server::{Cxp, McpResult};

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct NoArgs {}

#[tool_router(router = org_router, vis = "pub(crate)")]
impl Cxp {
    #[tool(
        name = "whoami",
        description = "Who this token says you are on Savvagent CXP: your user, the one \
                       organization it opens, your role there, the scopes it carries, and this \
                       service's meters and the org's Savvagent CXP budget (null if the \
                       platform cannot say right now; call usage later). Call this first in a \
                       session. Free.",
        annotations(read_only_hint = true)
    )]
    pub async fn whoami(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<Json<out::WhoAmI>, ErrorData> {
        let caller = self.caller(&parts)?;
        let member = self
            .platform()
            .member(caller.org_id.as_uuid(), caller.user_id.as_uuid())
            .await
            .map_err(|e| cxp_core::Error::platform_for(caller.org_id, e))
            .mcp()?;
        // Identity does not depend on billing: if the platform cannot report
        // usage right now, answer without it rather than failing the call.
        let usage = match self.meter().report(caller.org_id).await {
            Ok(status) => Some(status),
            Err(e) => {
                tracing::warn!(
                    code = e.code(),
                    error = %e,
                    source = ?std::error::Error::source(&e),
                    "whoami: the usage lookup failed; answering without it"
                );
                None
            }
        };

        let user = member.as_ref().map(|m| &m.user);
        let org = member.as_ref().map(|m| &m.org);
        Ok(Json(out::WhoAmI {
            service: cxp_core::scopes::RESOURCE_NAME,
            user: out::UserOut {
                id: caller.user_id,
                email: user.and_then(|u| u.email.clone()),
                name: user.and_then(|u| u.name.clone()),
            },
            org: out::OrgOut {
                id: caller.org_id,
                slug: org.map(|o| o.slug.clone()),
                name: org.map(|o| o.name.clone()),
                plan: org.map(|o| o.plan.clone()),
            },
            role: member.as_ref().map(|m| m.role.into()),
            token: out::TokenOut {
                kind: match caller.kind {
                    savvagent_resource::TokenKind::Oauth => "oauth",
                    savvagent_resource::TokenKind::Pat => "pat",
                },
                can_read: caller.can_read(),
                can_write: caller.can_write(),
                client_id: caller.client_id,
                scopes: caller.scopes,
                expires_at: caller.expires_at,
            },
            usage,
        }))
    }

    #[tool(
        name = "usage",
        description = "This service's own meters this month (otto-cxp/surveys_sent and \
                       otto-cxp/responses_collected), the organization's Savvagent CXP budget, and \
                       `blockedReason` (budget_exceeded or payment_required while billed calls \
                       are refused, else null). Only send_survey, submit_feedback, and \
                       submit_survey_response add to them. Free. If `warning` is true, tell the \
                       person you are working for.",
        annotations(read_only_hint = true)
    )]
    pub async fn usage(
        &self,
        Extension(parts): Extension<http::request::Parts>,
        Parameters(_): Parameters<NoArgs>,
    ) -> Result<Json<out::UsageOut>, ErrorData> {
        let caller = self.caller(&parts)?;
        let usage = self.meter().report(caller.org_id).await.mcp()?;
        Ok(Json(out::UsageOut { usage }))
    }
}
