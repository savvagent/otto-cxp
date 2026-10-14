//! Errors from otto-cxp's domain crate.
//!
//! Every message here may be read by an agent deciding its next call, so each
//! one says what was wrong and, where there is one, what to do instead.
//! [`Error::code`] is the stable machine-readable code MCP and REST put on the
//! wire; agents branch on it, never on text.
//!
//! `Display` never includes database or platform text: a constraint name, a
//! query fragment, or a platform response body is server detail, and the
//! `Display` of an error is what reaches a caller. The wrapped error stays
//! available through `std::error::Error::source` for logs.

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No row of this kind (e.g. `"customer"`, `"ticket"`) with that id or key
    /// in this organization.
    #[error("no {kind} with that id in this organization")]
    NotFound { kind: &'static str },

    /// The caller's arguments are wrong; the message says how.
    #[error("{0}")]
    Invalid(String),

    /// The request conflicts with existing state (a duplicate email, say); the
    /// message says what.
    #[error("{0}")]
    Conflict(String),

    /// A catalog entity named a team the platform does not know in this
    /// organization.
    #[error(
        "no team with that id in this organization; list your teams \
         (GET /api/v1/me/teams) and pass one of their ids"
    )]
    UnknownTeam,

    /// The org was deleted, or the caller removed from it, since the
    /// credential was last checked.
    #[error("access to this organization has been revoked; sign in again")]
    AccessRevoked,

    #[error(
        "this organization has used all {included} operations its {plan} plan includes this \
         month ({used} so far), so {tool} was refused. Reads still work. Upgrade at {upgrade_url}"
    )]
    QuotaExceeded {
        tool: String,
        used: i64,
        included: i64,
        plan: String,
        upgrade_url: String,
    },

    /// A dependency (the platform, or the database under contention) could not
    /// serve the request right now; nothing was changed.
    #[error("otto-cxp could not complete this right now; nothing was changed. Retry shortly")]
    Unavailable,

    /// A call to the otto platform failed. The text is ours, never the
    /// platform's.
    #[error("the otto platform could not be reached; nothing was changed. Retry shortly")]
    Platform(#[from] otto_resource::Error),

    /// The tenant substrate failed. Only its `Invalid` text, which describes
    /// the caller's request, is shown.
    #[error("{}", tenant_message(.0))]
    Tenant(#[from] otto_tenant::Error),

    /// A database failure. A data exception the caller's input caused (a value
    /// too long for its column, out of range, or malformed; SQLSTATE class
    /// `22`) or a check-constraint violation (`23514`) reads and codes as
    /// `invalid_argument`, however the error was constructed; anything else is
    /// `internal`.
    #[error("{}", database_message(.0))]
    Database(#[from] sqlx::Error),
}

/// What a caller is told of any server-side failure. Shared by every surface
/// so the wording cannot drift.
pub const INTERNAL_MESSAGE: &str =
    "otto-cxp hit an internal error; retry, and report it if it persists";

fn database_message(e: &sqlx::Error) -> &'static str {
    if is_input_rejection(e) {
        "a value is malformed, out of range, or too long for its field; \
         check the arguments and try again"
    } else {
        INTERNAL_MESSAGE
    }
}

/// Whether Postgres refused the statement because of a value the caller
/// supplied: a data exception (class `22`: too long, out of range, bad
/// format) or a check-constraint violation (`23514`, e.g. a rating outside
/// 1..=5). Retrying such a call unchanged can never succeed.
fn is_input_rejection(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => db
            .code()
            .is_some_and(|code| code.starts_with("22") || code == "23514"),
        _ => false,
    }
}

fn tenant_message(e: &otto_tenant::Error) -> String {
    match e {
        otto_tenant::Error::Invalid(msg) => msg.clone(),
        _ => INTERNAL_MESSAGE.to_string(),
    }
}

impl Error {
    /// A stable, machine-readable code. Agents branch on this, never on text.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotFound { .. } => "not_found",
            Error::Invalid(_) | Error::Tenant(otto_tenant::Error::Invalid(_)) => "invalid_argument",
            Error::Database(e) if is_input_rejection(e) => "invalid_argument",
            Error::Conflict(_) => "conflict",
            Error::UnknownTeam => "unknown_team",
            Error::AccessRevoked => "access_revoked",
            Error::QuotaExceeded { .. } => "quota_exceeded",
            Error::Unavailable | Error::Platform(_) => "unavailable",
            Error::Tenant(_) | Error::Database(_) => "internal",
        }
    }

    /// Whether repeating the same call unchanged could succeed.
    pub fn retriable(&self) -> bool {
        match self {
            Error::Database(e) => !is_input_rejection(e),
            Error::Unavailable => true,
            Error::Platform(p) => p.is_retriable(),
            Error::Tenant(otto_tenant::Error::Db(_)) => true,
            _ => false,
        }
    }

    /// Whether this failure is the kind that passes on its own: contention
    /// (serialization failure, deadlock, a lock or statement timeout), a
    /// connection or pool problem, or the platform being unreachable. Narrower
    /// than [`Error::retriable`], which tells an agent any database error is
    /// worth one more try: a background job retrying on a schedule must not
    /// keep retrying a constraint violation or a missing table.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Database(e) | Error::Tenant(otto_tenant::Error::Db(e)) => db_is_transient(e),
            Error::Unavailable => true,
            Error::Platform(p) => p.is_retriable(),
            _ => false,
        }
    }

    /// Whether this is the server's fault rather than a description of the
    /// caller's request. Callers log these with their source.
    pub fn is_internal(&self) -> bool {
        match self {
            Error::Database(e) => !is_input_rejection(e),
            Error::Platform(_) => true,
            Error::Tenant(t) => !matches!(t, otto_tenant::Error::Invalid(_)),
            _ => false,
        }
    }
}

/// SQLSTATEs that a later attempt can avoid: serialization_failure,
/// deadlock_detected, lock_not_available (`lock_timeout`), and query_canceled
/// (`statement_timeout`).
const TRANSIENT_SQLSTATES: &[&str] = &["40001", "40P01", "55P03", "57014"];

fn db_is_transient(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::PoolTimedOut | sqlx::Error::PoolClosed | sqlx::Error::Io(_) => true,
        sqlx::Error::Database(db) => db
            .code()
            .is_some_and(|code| TRANSIENT_SQLSTATES.contains(&code.as_ref())),
        _ => false,
    }
}

/// Map a unique violation to `to`, anything else to [`Error::Database`]. For
/// domain writes whose natural keys are unique per org (a customer's email).
pub fn on_unique(e: sqlx::Error, to: impl FnOnce() -> Error) -> Error {
    match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => to(),
        _ => Error::Database(e),
    }
}

/// Map a foreign-key violation to `to`, anything else to [`Error::Database`].
/// A composite `(org_id, x_id)` foreign key refuses another org's id exactly
/// as it refuses a missing one, so `to` is usually [`Error::NotFound`].
pub fn on_foreign_key(e: sqlx::Error, to: impl FnOnce() -> Error) -> Error {
    match &e {
        sqlx::Error::Database(db) if db.is_foreign_key_violation() => to(),
        _ => Error::Database(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    /// A database error with a chosen SQLSTATE and message: sqlx's own
    /// Postgres error cannot be built outside sqlx.
    #[derive(Debug)]
    struct Sqlstate(&'static str);

    const DB_TEXT: &str =
        "duplicate key value violates unique constraint \"customers_org_id_email_key\"";

    impl std::fmt::Display for Sqlstate {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(DB_TEXT)
        }
    }

    impl std::error::Error for Sqlstate {}

    impl sqlx::error::DatabaseError for Sqlstate {
        fn message(&self) -> &str {
            DB_TEXT
        }
        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.0))
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            match self.0 {
                "23505" => sqlx::error::ErrorKind::UniqueViolation,
                "23503" => sqlx::error::ErrorKind::ForeignKeyViolation,
                _ => sqlx::error::ErrorKind::Other,
            }
        }
    }

    fn db(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(Sqlstate(code)))
    }

    fn quota() -> Error {
        Error::QuotaExceeded {
            tool: "submit_feedback".into(),
            used: 10,
            included: 10,
            plan: "free".into(),
            upgrade_url: "https://otto.savvagent.com/billing".into(),
        }
    }

    /// A value the caller supplied that Postgres refused (too long, out of
    /// range, malformed, or failing a check constraint) is the caller's to
    /// fix: `invalid_argument`, not retriable, not internal, and still no
    /// database text.
    #[test]
    fn input_the_database_refuses_is_invalid_not_internal() {
        for code in ["22001", "22003", "22007", "22P02", "23514"] {
            let e = Error::Database(db(code));
            assert_eq!(e.code(), "invalid_argument", "{code}");
            assert!(!e.retriable(), "{code}");
            assert!(!e.is_internal(), "{code}");
            assert!(!e.to_string().contains("constraint"), "{code}");
        }
        for code in ["23505", "23503", "42P01"] {
            let e = Error::Database(db(code));
            assert_eq!(e.code(), "internal", "{code}");
            assert!(e.is_internal(), "{code}");
        }
    }

    /// These codes are the wire contract MCP and REST clients branch on.
    #[test]
    fn every_variant_has_its_wire_code() {
        let cases: Vec<(Error, &str)> = vec![
            (Error::NotFound { kind: "customer" }, "not_found"),
            (Error::Invalid("bad".into()), "invalid_argument"),
            (
                Error::Tenant(otto_tenant::Error::Invalid("bad".into())),
                "invalid_argument",
            ),
            (Error::Conflict("dup".into()), "conflict"),
            (Error::UnknownTeam, "unknown_team"),
            (Error::AccessRevoked, "access_revoked"),
            (quota(), "quota_exceeded"),
            (Error::Unavailable, "unavailable"),
            (
                Error::Platform(otto_resource::Error::Unauthorized),
                "unavailable",
            ),
            (
                Error::Tenant(otto_tenant::Error::Db(db("23505"))),
                "internal",
            ),
            (Error::Database(db("23505")), "internal"),
        ];
        for (err, code) in cases {
            assert_eq!(err.code(), code, "{err:?}");
        }
    }

    #[test]
    fn display_never_includes_database_or_platform_text() {
        let platform_body = "stack trace from the platform";
        for err in [
            Error::Database(db("23505")),
            Error::Tenant(otto_tenant::Error::Db(db("23505"))),
            Error::Platform(otto_resource::Error::Status {
                status: 500,
                body: platform_body.into(),
            }),
        ] {
            let shown = err.to_string();
            assert!(!shown.contains("customers_org_id_email_key"), "{shown}");
            assert!(!shown.contains("duplicate key"), "{shown}");
            assert!(!shown.contains(platform_body), "{shown}");
            assert!(err.is_internal(), "{err:?}");
        }
        // The caller's own mistakes are shown as written.
        assert_eq!(
            Error::Invalid("rating must be 1-5".into()).to_string(),
            "rating must be 1-5"
        );
        assert_eq!(
            Error::NotFound { kind: "ticket" }.to_string(),
            "no ticket with that id in this organization"
        );
        assert!(!Error::Invalid("x".into()).is_internal());
        assert!(!Error::Tenant(otto_tenant::Error::Invalid("x".into())).is_internal());
    }

    #[test]
    fn retriable_marks_what_an_agent_may_simply_retry() {
        for yes in [
            Error::Database(db("23505")),
            Error::Database(sqlx::Error::PoolTimedOut),
            Error::Tenant(otto_tenant::Error::Db(db("40001"))),
            Error::Unavailable,
            Error::Platform(otto_resource::Error::Status {
                status: 503,
                body: String::new(),
            }),
        ] {
            assert!(yes.retriable(), "{yes:?}");
        }
        for no in [
            Error::NotFound { kind: "customer" },
            Error::Invalid("bad".into()),
            Error::Conflict("dup".into()),
            Error::UnknownTeam,
            Error::AccessRevoked,
            quota(),
            Error::Platform(otto_resource::Error::Unauthorized),
            Error::Tenant(otto_tenant::Error::Invalid("bad".into())),
        ] {
            assert!(!no.retriable(), "{no:?}");
        }
    }

    #[test]
    fn only_contention_cancellation_and_connection_failures_are_transient() {
        let io = || std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset");
        for transient in [
            Error::Database(sqlx::Error::PoolTimedOut),
            Error::Database(sqlx::Error::PoolClosed),
            Error::Database(sqlx::Error::Io(io())),
            Error::Database(db("40001")),
            Error::Database(db("40P01")),
            Error::Database(db("55P03")),
            Error::Database(db("57014")),
            Error::Tenant(otto_tenant::Error::Db(db("40P01"))),
            Error::Unavailable,
        ] {
            assert!(transient.is_transient(), "{transient:?}");
        }
        for permanent in [
            Error::Database(db("23505")),
            Error::Database(db("42P01")),
            Error::Database(db("42501")),
            Error::Database(sqlx::Error::RowNotFound),
            Error::Tenant(otto_tenant::Error::Db(db("23505"))),
            Error::AccessRevoked,
            Error::Invalid("bad".into()),
        ] {
            assert!(!permanent.is_transient(), "{permanent:?}");
        }
    }

    #[test]
    fn constraint_violations_map_to_domain_errors() {
        let e = on_unique(db("23505"), || Error::Conflict("dup".into()));
        assert_eq!(e.code(), "conflict");
        let e = on_unique(db("23503"), || Error::Conflict("dup".into()));
        assert_eq!(e.code(), "internal");

        let e = on_foreign_key(db("23503"), || Error::NotFound { kind: "customer" });
        assert_eq!(e.code(), "not_found");
        let e = on_foreign_key(db("23505"), || Error::NotFound { kind: "customer" });
        assert_eq!(e.code(), "internal");
    }
}
