//! The tool surface, split by domain. `rmcp`'s [`ToolRouter`] adds, so the
//! per-file routers are combined in [`router`].
//!
//! Tool names and input field names are a stable contract with agents.
//! There are no team tools: teams belong to the platform. Paged tools take an
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
    /// The UUID of the record the tool names.
    pub id: String,
}

/// A customer, named however the agent knows them. The field is `id`, as in
/// [`IdParam`], so the wire shape is unchanged.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CustomerIdParam {
    /// The customer: a UUID, an exact email, or a name (matched exactly,
    /// case-insensitively, else by a unique partial match of the name or
    /// email; several matches are refused, and search_customers lists them).
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
            // `self.charge(&mut tx, &caller, "name", &warmed)`: the third
            // argument, the tool's name.
            let last = call.split(',').nth(2).unwrap_or("").trim();
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
    fn exactly_the_three_billable_tools_charge_once_each() {
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
        // And only they look up at the platform whether billing is blocked.
        let warmed: usize = SOURCES
            .iter()
            .map(|(_, src)| src.matches("self.warm_quota(").count())
            .sum();
        assert_eq!(warmed, billable.len());
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
                .find(&format!(".charge(&mut tx, &caller, \"{tool}\", &warmed)"))
                .unwrap_or_else(|| panic!("{tool} does not charge itself"));
            let tx = body.find("self.tx(&caller)").expect("opens a transaction");
            let commit = body.find("tx.commit()").expect("commits");
            assert!(
                tx < charge && charge < commit,
                "{tool} charges outside its transaction"
            );
            // The billing lookup is a network call: made before the
            // transaction holds a pooled connection.
            let warm = body
                .find("let warmed = self.warm_quota(&caller)")
                .unwrap_or_else(|| panic!("{tool} does not look billing up"));
            assert!(warm < tx, "{tool} looks billing up inside its transaction");
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

    /// The files that define tools; each tool's body is cut out of its own
    /// file, so the order here does not matter.
    const TOOL_SOURCES: &[(&str, &str)] = &[
        ("org.rs", include_str!("org.rs")),
        ("cdp.rs", include_str!("cdp.rs")),
        ("support.rs", include_str!("support.rs")),
        ("feedback.rs", include_str!("feedback.rs")),
    ];

    /// The source of one tool: from its `#[tool(` attribute to the next one
    /// (or the end of its file). Exactly one file may define it.
    fn tool_body(name: &str) -> &'static str {
        let needle = format!("name = \"{name}\"");
        let mut found = TOOL_SOURCES.iter().filter_map(|(file, src)| {
            let at = src.find(&needle)?;
            let start = src[..at].rfind("#[tool(").unwrap_or(at);
            let rest = &src[start..];
            let end = rest[1..].find("#[tool(").map_or(rest.len(), |i| i + 1);
            Some((*file, &rest[..end]))
        });
        let (_, body) = found
            .next()
            .unwrap_or_else(|| panic!("{name} has no #[tool]"));
        assert!(found.next().is_none(), "{name} is defined twice");
        body
    }

    #[test]
    fn every_scoped_tool_checks_the_right_scope_before_touching_data() {
        for name in NAMES {
            let body = tool_body(name);
            let read = body.find("caller.require_read()");
            let write = body.find("caller.require_write()");
            let tx = body.find("self.tx(");
            let check = if UNSCOPED.contains(name) {
                assert!(
                    read.is_none() && write.is_none(),
                    "{name} should check no scope"
                );
                assert!(tx.is_none(), "{name} should open no transaction");
                continue;
            } else if WRITES.contains(name) {
                assert!(read.is_none(), "{name} should require cxp:write, not read");
                write.unwrap_or_else(|| panic!("{name} should require cxp:write"))
            } else {
                assert!(write.is_none(), "{name} should require read, not cxp:write");
                read.unwrap_or_else(|| panic!("{name} should require read"))
            };
            if let Some(tx) = tx {
                assert!(
                    check < tx,
                    "{name} opens its transaction before its scope check"
                );
            }
        }
    }

    #[test]
    fn create_survey_documents_the_question_shape_the_console_renders() {
        let tool = router()
            .list_all()
            .into_iter()
            .find(|t| t.name == "create_survey")
            .unwrap();
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(
            description.contains(crate::prompts::SURVEY_QUESTIONS_EXAMPLE),
            "{description}"
        );
        assert!(!description.contains("\"type\""), "{description}");
        let schema = serde_json::to_string(&tool.input_schema).unwrap();
        assert!(schema.contains("questionType"), "{schema}");
        assert!(!schema.contains("\\\"type\\\": \\\"nps"), "{schema}");
    }

    #[test]
    fn annotations_agree_with_the_write_list() {
        for tool in router().list_all() {
            let name = tool.name.to_string();
            let a = tool
                .annotations
                .unwrap_or_else(|| panic!("{name} has no annotations"));
            if WRITES.contains(&name.as_str()) {
                assert_eq!(a.read_only_hint, Some(false), "{name}");
                assert_eq!(
                    a.destructive_hint,
                    Some(false),
                    "{name}: nothing here deletes"
                );
            } else {
                assert_eq!(a.read_only_hint, Some(true), "{name}");
            }
        }
    }
}
