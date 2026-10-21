//! Turning domain failures into something an agent can act on.
//!
//! The branch point is machine-readable: `data.code` is stable
//! (`insufficient_scope`, `not_found`, `invalid_argument`, `conflict`,
//! `unknown_team`, `access_revoked`, `budget_exceeded`, `payment_required`,
//! `unavailable`, `query_too_broad`, `internal`, and `unauthenticated` for a
//! surface mounted
//! without its middleware) and `data.retriable` says whether repeating the call unchanged
//! could succeed.
//!
//! `cxp_core::Error`'s `Display` never carries database or platform text, so
//! the message is passed through as is; server-side failures are logged here
//! with their source, which does.
//!
//! **A revocation found mid-request is a JSON-RPC error, by design.** The
//! bearer middleware answers a token that is already dead with HTTP `401` and
//! `WWW-Authenticate`. But the org can be deleted, or the caller removed,
//! after the token was introspected and before the tool's transaction opens;
//! by then the request is a JSON-RPC call in flight, so it fails as one:
//! `INVALID_REQUEST` with `data.code` `access_revoked` and no
//! `WWW-Authenticate` header. An agent branches on the code, and its next
//! request meets the `401` in the usual way.

use cxp_core::auth::MissingScope;
use cxp_core::Error;
use rmcp::model::{ErrorCode, ErrorData};

fn data(code: &str, retriable: bool) -> Option<serde_json::Value> {
    Some(serde_json::json!({ "code": code, "retriable": retriable }))
}

/// A domain failure on the wire.
pub fn from_core(e: &Error) -> ErrorData {
    if e.is_internal() {
        tracing::error!(
            code = e.code(),
            error = %e.log_detail(),
            source = ?std::error::Error::source(e),
            "server-side failure surfaced to an MCP caller"
        );
    }
    let rpc = match e.code() {
        // Well-formed calls the server declined (a platform that no longer
        // knows the org reads as access_revoked too; a billable call while
        // billing is blocked).
        "access_revoked" | "budget_exceeded" | "payment_required" => ErrorCode::INVALID_REQUEST,
        _ if e.is_internal() => ErrorCode::INTERNAL_ERROR,
        "unavailable" => ErrorCode::INTERNAL_ERROR,
        _ => ErrorCode::INVALID_PARAMS,
    };
    ErrorData::new(rpc, e.to_string(), data(e.code(), e.retriable()))
}

/// A tenant-substrate failure: wrapped as the domain does, so only its
/// `Invalid` text is ever shown.
pub fn from_tenant(e: otto_tenant::Error) -> ErrorData {
    from_core(&Error::Tenant(e))
}

/// A platform lookup (`whoami`'s member, `usage`'s report) that failed.
pub fn from_platform(e: otto_resource::Error) -> ErrorData {
    from_core(&Error::Platform(e))
}

/// A token without the scope the call needs. Actionable: the agent must
/// re-authorize asking for it, so the message names it.
pub fn from_scope(e: &MissingScope) -> ErrorData {
    ErrorData::new(
        ErrorCode::INVALID_REQUEST,
        format!(
            "this token lacks the {} scope; re-authorize this server requesting it (the scopes \
             it knows are listed at /.well-known/oauth-protected-resource)",
            e.0
        ),
        data("insufficient_scope", false),
    )
}

/// A caller's argument that is wrong, outside any domain call (an unknown
/// prompt, a missing prompt argument, a malformed resource URI).
pub fn invalid(message: impl Into<String>) -> ErrorData {
    from_core(&Error::Invalid(message.into()))
}

/// A server-side failure with no domain error behind it (an output that could
/// not be encoded). Logged with its cause; the caller gets the code and
/// nothing else.
pub fn internal(what: &'static str, cause: &dyn std::fmt::Display) -> ErrorData {
    tracing::error!(code = "internal", error = %cause, "{what}");
    ErrorData::new(
        ErrorCode::INTERNAL_ERROR,
        cxp_core::error::INTERNAL_MESSAGE,
        data("internal", false),
    )
}

/// A resource read that named something this organization does not have:
/// MCP's resource-not-found (-32002), still with `data.code` `not_found`.
/// Anything else passes through unchanged.
pub fn as_resource_not_found(mut e: ErrorData) -> ErrorData {
    if e.data
        .as_ref()
        .and_then(|d| d.get("code"))
        .and_then(|c| c.as_str())
        == Some("not_found")
    {
        e.code = ErrorCode::RESOURCE_NOT_FOUND;
    }
    e
}

/// Reached only if the MCP surface was mounted without its middleware.
pub fn unauthenticated() -> ErrorData {
    ErrorData::internal_error(
        "this request carried no authenticated principal, which means the MCP surface was \
         mounted without its authentication middleware; this is a server misconfiguration, \
         not a problem with your call",
        data("unauthenticated", false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cxp_core::error::BlockedReason;

    fn pg_error() -> Error {
        Error::Database(sqlx_like())
    }

    fn sqlx_like() -> sqlx::Error {
        sqlx::Error::Protocol("relation \"customers\" violates secret_constraint_name".into())
    }

    fn blocked(reason: BlockedReason) -> Error {
        Error::BillingBlocked {
            reason,
            tool: "send_survey".into(),
            url: "https://otto.example.com".into(),
        }
    }

    /// A billable call refused while billing is blocked is a well-formed call
    /// the server declined, and repeating it cannot help.
    #[test]
    fn billing_refusals_are_invalid_requests_that_are_not_retriable() {
        for (reason, code) in [
            (BlockedReason::BudgetExceeded, "budget_exceeded"),
            (BlockedReason::PaymentRequired, "payment_required"),
        ] {
            let wire = from_core(&blocked(reason));
            assert_eq!(wire.code, ErrorCode::INVALID_REQUEST, "{code}");
            let d = wire.data.expect("data");
            assert_eq!(d["code"], code);
            assert_eq!(d["retriable"], false, "{code}");
            assert!(wire.message.contains("send_survey"), "{}", wire.message);
        }
    }

    #[test]
    fn every_error_carries_its_code_and_retriable_flag() {
        for e in [
            Error::NotFound { kind: "customer" },
            Error::Invalid("bad".into()),
            Error::Conflict("dup".into()),
            Error::UnknownTeam,
            Error::AccessRevoked,
            blocked(BlockedReason::BudgetExceeded),
            blocked(BlockedReason::PaymentRequired),
            Error::Unavailable,
            Error::QueryTooBroad,
            pg_error(),
            Error::Tenant(otto_tenant::Error::Config("secret".into())),
        ] {
            let wire = from_core(&e);
            let d = wire.data.expect("data");
            assert_eq!(d["code"], e.code(), "{e:?}");
            assert_eq!(d["retriable"], e.retriable(), "{e:?}");
        }
    }

    /// `data.retriable` is true only for a platform failure that can pass,
    /// and agrees with the code.
    #[test]
    fn platform_failures_carry_a_consistent_code_and_retriable_flag() {
        let wire = |e: otto_resource::Error| {
            let w = from_platform(e);
            let d = w.data.unwrap();
            (w.code, d["code"].clone(), d["retriable"].clone())
        };
        assert_eq!(
            wire(otto_resource::Error::Status {
                status: 503,
                body: "down".into()
            }),
            (ErrorCode::INTERNAL_ERROR, "unavailable".into(), true.into())
        );
        assert_eq!(
            wire(otto_resource::Error::NotFound),
            (
                ErrorCode::INVALID_REQUEST,
                "access_revoked".into(),
                false.into()
            )
        );
        assert_eq!(
            wire(otto_resource::Error::Unauthorized),
            (ErrorCode::INTERNAL_ERROR, "internal".into(), false.into())
        );
    }

    #[test]
    fn the_codes_on_the_wire_are_the_documented_ones() {
        let code = |e: Error| from_core(&e).data.unwrap()["code"].clone();
        assert_eq!(code(Error::NotFound { kind: "ticket" }), "not_found");
        assert_eq!(code(Error::Invalid("x".into())), "invalid_argument");
        assert_eq!(code(Error::Conflict("x".into())), "conflict");
        assert_eq!(code(Error::UnknownTeam), "unknown_team");
        assert_eq!(code(Error::AccessRevoked), "access_revoked");
        assert_eq!(code(Error::Unavailable), "unavailable");
        assert_eq!(code(Error::QueryTooBroad), "query_too_broad");
        assert_eq!(code(pg_error()), "internal");
        assert_eq!(
            from_scope(&MissingScope("cxp:write")).data.unwrap()["code"],
            "insufficient_scope"
        );
        assert_eq!(unauthenticated().data.unwrap()["code"], "unauthenticated");
    }

    #[test]
    fn database_and_tenant_text_never_reach_the_wire() {
        let db = from_core(&pg_error());
        assert_eq!(db.code, ErrorCode::INTERNAL_ERROR);
        assert!(!db.message.contains("customers"), "{}", db.message);
        assert!(!db.message.contains("secret"), "{}", db.message);

        let t = from_tenant(otto_tenant::Error::Config("secret key path".into()));
        assert!(!t.message.contains("secret"), "{}", t.message);
        assert_eq!(t.data.unwrap()["code"], "internal");

        let invalid = from_tenant(otto_tenant::Error::Invalid("bad org".into()));
        assert_eq!(invalid.message, "bad org");
        assert_eq!(invalid.data.unwrap()["code"], "invalid_argument");
    }

    #[test]
    fn the_scope_error_names_the_missing_scope() {
        let e = from_scope(&MissingScope("cxp:write"));
        assert!(e.message.contains("cxp:write"), "{}", e.message);
        // One instruction: re-authorize, not also "obtain a token".
        assert!(!e.message.contains("obtain"), "{}", e.message);
        assert_eq!(
            e.message.matches("re-authorize").count(),
            1,
            "{}",
            e.message
        );
        assert_eq!(e.data.unwrap()["retriable"], false);
    }

    #[test]
    fn helper_errors_carry_codes_and_not_found_reads_are_resource_not_found() {
        assert_eq!(invalid("x").data.unwrap()["code"], "invalid_argument");
        let i = internal("encoding failed", &"key must be a string");
        assert_eq!(i.code, ErrorCode::INTERNAL_ERROR);
        assert_eq!(i.data.unwrap()["code"], "internal");

        let nf = as_resource_not_found(from_core(&Error::NotFound { kind: "customer" }));
        assert_eq!(nf.code, ErrorCode::RESOURCE_NOT_FOUND);
        assert_eq!(nf.data.unwrap()["code"], "not_found");
        let other = as_resource_not_found(invalid("x"));
        assert_eq!(other.code, ErrorCode::INVALID_PARAMS);
    }
}
