//! The tool surface, split by domain. `rmcp`'s [`ToolRouter`] adds, so the
//! per-file routers are combined in [`router`].
//!
//! Tool names and input field names are the pre-platform server's, minus its
//! five team tools (teams belong to the platform now). Paged tools take an
//! optional `limit` and `offset`; every list tool returns
//! `{"items": [...], "total": n}` (see [`out::List`]).
//!
//! **Scopes:** a read needs `cxp:read` or `cxp:write`; a write needs
//! `cxp:write`; `whoami` and `usage` need neither.
//!
//! **Metering:** exactly the tools in `cxp_core::usage::BILLABLE` call
//! [`Cxp::charge`]; the tests below hold the source to that.

use rmcp::handler::server::tool::ToolRouter;
use rmcp::model::ErrorData;
use serde::Deserialize;
use uuid::Uuid;

use crate::server::{Cxp, McpResult};

pub mod cdp;
pub mod feedback;
pub mod org;
pub mod out;
pub mod support;

pub fn router() -> ToolRouter<Cxp> {
    Cxp::org_router() + Cxp::cdp_router() + Cxp::support_router() + Cxp::feedback_router()
}

/// Every tool name, in one place for tests and documentation.
pub const NAMES: &[&str] = &[
    // org
    "whoami",
    "usage",
    // customer data
    "create_customer",
    "list_customers",
    "update_customer",
    "get_customer",
    "search_customers",
    "record_interaction",
    "get_journey",
    "create_segment",
    "get_segment_members",
    "get_customer_360",
    // support
    "create_ticket",
    "update_ticket",
    "get_ticket",
    "list_tickets",
    "close_ticket",
    "search_knowledge_base",
    "start_chat_session",
    "send_chat_message",
    "get_chat_messages",
    "end_chat_session",
    // feedback
    "get_survey",
    "submit_survey_response",
    "create_survey",
    "send_survey",
    "get_survey_responses",
    "submit_feedback",
    "get_feedback",
    "calculate_nps",
    "calculate_csat",
    "analyze_sentiment",
];

/// The tools that change something and so need `cxp:write`. Every other tool
/// but `whoami` and `usage` needs read.
pub const WRITES: &[&str] = &[
    "create_customer",
    "update_customer",
    "record_interaction",
    "create_segment",
    "create_ticket",
    "update_ticket",
    "close_ticket",
    "start_chat_session",
    "send_chat_message",
    "end_chat_session",
    "submit_survey_response",
    "create_survey",
    "send_survey",
    "submit_feedback",
];

/// The tools that check no scope.
pub const UNSCOPED: &[&str] = &["whoami", "usage"];

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdParam {
    /// The id. Tools that take a customer also accept an exact email or a name.
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QueryParam {
    pub query: String,
}

/// Parse a uuid argument, or say which one was wrong.
pub(crate) fn uuid(field: &str, value: &str) -> Result<Uuid, ErrorData> {
    Uuid::parse_str(value.trim())
        .map_err(|_| cxp_core::Error::Invalid(format!("{field} must be a UUID")))
        .mcp()
}

pub(crate) fn optional_uuid(field: &str, value: Option<&str>) -> Result<Option<Uuid>, ErrorData> {
    value.map(|v| uuid(field, v)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCES: &[(&str, &str)] = &[
        ("org.rs", include_str!("org.rs")),
        ("cdp.rs", include_str!("cdp.rs")),
        ("support.rs", include_str!("support.rs")),
        ("feedback.rs", include_str!("feedback.rs")),
        ("../server.rs", include_str!("../server.rs")),
        ("../resources.rs", include_str!("../resources.rs")),
        ("../prompts.rs", include_str!("../prompts.rs")),
    ];

    #[test]
    fn the_names_match_the_router_and_there_are_thirty_two() {
        let mut routed: Vec<String> = router()
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        let mut expected: Vec<String> = NAMES.iter().map(|s| s.to_string()).collect();
        routed.sort();
        expected.sort();
        assert_eq!(routed, expected);
        assert_eq!(NAMES.len(), 32);
    }

    #[test]
    fn every_billable_name_is_a_write_tool() {
        for billable in cxp_core::usage::BILLABLE {
            assert!(
                NAMES.contains(billable),
                "{billable} is billed but is not a tool"
            );
            assert!(
                WRITES.contains(billable),
                "{billable} is billed but not a write"
            );
        }
        for w in WRITES.iter().chain(UNSCOPED) {
            assert!(NAMES.contains(w), "{w} is not a tool");
        }
    }

    #[test]
    fn the_team_tools_are_gone() {
        for t in [
            "create_team",
            "get_team",
            "list_teams",
            "update_team",
            "delete_team",
        ] {
            assert!(!NAMES.contains(&t), "{t} should not be served");
        }
    }

    /// The tool names the source passes to `charge`, in order. A charge
    /// whose argument is not a string literal is reported as `"<dynamic>"`,
    /// which fails the test below: billing must be visible in the source.
    fn charged_in(src: &str) -> Vec<String> {
        let mut out = vec![];
        let mut rest = src;
        while let Some(at) = rest.find(".charge(") {
            rest = &rest[at + ".charge(".len()..];
            let call = &rest[..rest.find(')').unwrap_or(rest.len())];
            // `self.charge(&mut tx, &caller, "name")`: the last argument.
            let last = call.rsplit(',').next().unwrap_or("").trim();
            let name = last
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .map(str::to_string)
                .unwrap_or_else(|| "<dynamic>".to_string());
            out.push(name);
        }
        out
    }

    #[test]
    fn exactly_the_four_billable_tools_charge_once_each() {
        let mut charged: Vec<String> = SOURCES
            .iter()
            .flat_map(|(file, src)| {
                // The one `.charge(` outside a tool is `Cxp::charge`'s own call
                // into `Meter::charge`, which passes the tool through.
                let calls = charged_in(src);
                if *file == "../server.rs" {
                    assert_eq!(calls, ["<dynamic>"], "server.rs should only forward");
                    vec![]
                } else {
                    calls
                }
            })
            .collect();
        charged.sort();
        let mut billable: Vec<String> = cxp_core::usage::BILLABLE
            .iter()
            .map(|s| s.to_string())
            .collect();
        billable.sort();
        assert_eq!(charged, billable);
    }

    #[test]
    fn each_billable_tool_charges_inside_its_own_body() {
        let src = include_str!("feedback.rs");
        for tool in cxp_core::usage::BILLABLE {
            let start = src
                .find(&format!("pub async fn {tool}("))
                .unwrap_or_else(|| panic!("{tool} not in feedback.rs"));
            let body = &src[start..];
            let end = body[1..]
                .find("pub async fn ")
                .map_or(body.len(), |i| i + 1);
            let body = &body[..end];
            let charge = body
                .find(&format!(".charge(&mut tx, &caller, \"{tool}\")"))
                .unwrap_or_else(|| panic!("{tool} does not charge itself"));
            let tx = body.find("self.tx(&caller)").expect("opens a transaction");
            let commit = body.find("tx.commit()").expect("commits");
            assert!(
                tx < charge && charge < commit,
                "{tool} charges outside its transaction"
            );
            // Before its write: the domain call of the same name follows the
            // charge, and does not precede it.
            assert!(
                !body[..charge].contains(&format!(".{tool}(")),
                "{tool} writes before it charges"
            );
            assert!(
                body[charge..].contains(&format!(".{tool}(")),
                "{tool}'s write not found after its charge"
            );
        }
    }

    #[test]
    fn every_scoped_tool_checks_the_right_scope() {
        let bodies: String = SOURCES[..4].iter().map(|(_, s)| *s).collect();
        for name in NAMES {
            let start = bodies
                .find(&format!("name = \"{name}\""))
                .unwrap_or_else(|| panic!("{name} has no #[tool]"));
            let body = &bodies[start..];
            let end = body[1..].find("#[tool(").map_or(body.len(), |i| i + 1);
            let body = &body[..end];
            let reads = body.contains("caller.require_read()");
            let writes = body.contains("caller.require_write()");
            if UNSCOPED.contains(name) {
                assert!(!reads && !writes, "{name} should check no scope");
            } else if WRITES.contains(name) {
                assert!(writes && !reads, "{name} should require cxp:write");
            } else {
                assert!(reads && !writes, "{name} should require read");
            }
        }
    }
}
