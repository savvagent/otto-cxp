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

    /// This could not be completed right now; nothing was changed, and the
    /// same call may succeed later. Raised for a platform lifecycle event this
    /// build parses but has no handler for (so the platform redelivers it
    /// until a release that handles it is deployed); platform failures are
    /// [`Error::Platform`].
    #[error("otto-cxp could not complete this right now; nothing was changed. Retry shortly")]
    Unavailable,

    /// A call to the otto platform failed. The text is ours, never the
    /// platform's, and depends on whether the failure can pass:
    ///
    /// - retriable (a transport failure, 5xx, or 429): `unavailable`, "retry
    ///   shortly";
    /// - the platform no longer knows the caller's org (`NotFound`):
    ///   `access_revoked`, as [`Error::AccessRevoked`];
    /// - anything else (this service's credential refused, a misconfigured
    ///   platform URL, an unreadable answer): `internal`. Retrying cannot
    ///   help, so the caller is not told to.
    #[error("{}", platform_message(.0))]
    Platform(#[from] otto_resource::Error),

    /// A server-side programming error (e.g. metering a tool that is not
    /// billable). The detail is for logs (`Debug`); callers see only
    /// [`INTERNAL_MESSAGE`].
    #[error("{}", INTERNAL_MESSAGE)]
    Internal(String),

    /// The tenant substrate failed. Its `Invalid` text, which describes the
    /// caller's request, is shown; its database failures read and code
    /// exactly as [`Error::Database`] does.
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

fn platform_message(e: &otto_resource::Error) -> &'static str {
    if e.is_retriable() {
        "the otto platform could not be reached; nothing was changed. Retry shortly"
    } else if matches!(e, otto_resource::Error::NotFound) {
        "access to this organization has been revoked; sign in again"
    } else {
        INTERNAL_MESSAGE
    }
}

fn tenant_message(e: &otto_tenant::Error) -> String {
    match e {
        otto_tenant::Error::Invalid(msg) => msg.clone(),
        otto_tenant::Error::Db(db) => database_message(db).to_string(),
        _ => INTERNAL_MESSAGE.to_string(),
    }
}

impl Error {
    /// The database error inside, whichever way it arrived: directly, or
    /// through the tenant substrate. Both are treated identically.
    fn db(&self) -> Option<&sqlx::Error> {
        match self {
            Error::Database(e) | Error::Tenant(otto_tenant::Error::Db(e)) => Some(e),
            _ => None,
        }
    }

    /// A stable, machine-readable code. Agents branch on this, never on text.
    pub fn code(&self) -> &'static str {
        if let Some(e) = self.db() {
            return if is_input_rejection(e) {
                "invalid_argument"
            } else {
                "internal"
            };
        }
        match self {
            Error::NotFound { .. } => "not_found",
            Error::Invalid(_) | Error::Tenant(otto_tenant::Error::Invalid(_)) => "invalid_argument",
            Error::Conflict(_) => "conflict",
            Error::UnknownTeam => "unknown_team",
            Error::AccessRevoked => "access_revoked",
            Error::QuotaExceeded { .. } => "quota_exceeded",
            Error::Unavailable => "unavailable",
            Error::Platform(p) if p.is_retriable() => "unavailable",
            Error::Platform(otto_resource::Error::NotFound) => "access_revoked",
            Error::Platform(_) | Error::Internal(_) | Error::Tenant(_) | Error::Database(_) => {
                "internal"
            }
        }
    }

    /// Whether repeating the same call unchanged could succeed.
    pub fn retriable(&self) -> bool {
        if let Some(e) = self.db() {
            return !is_input_rejection(e);
        }
        match self {
            Error::Unavailable => true,
            Error::Platform(p) => p.is_retriable(),
            _ => false,
        }
    }

    /// Whether this is the server's fault rather than a description of the
    /// caller's request. Callers log these with their source.
    pub fn is_internal(&self) -> bool {
        if let Some(e) = self.db() {
            return !is_input_rejection(e);
        }
        match self {
            Error::Platform(otto_resource::Error::NotFound) => false,
            Error::Platform(_) | Error::Internal(_) => true,
            Error::Tenant(t) => !matches!(t, otto_tenant::Error::Invalid(_)),
            _ => false,
        }
    }

    /// What a surface logs about a server-side failure: the `Display` plus
    /// the detail it hides from callers ([`Error::Internal`]'s message, the
    /// platform's or the database's own error).
    pub fn log_detail(&self) -> String {
        match self {
            Error::Internal(detail) => format!("internal: {detail}"),
            Error::Platform(p) => format!("platform: {p}"),
            Error::Tenant(t) => format!("tenant: {t}"),
            Error::Database(d) => format!("database: {d}"),
            other => other.to_string(),
        }
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

/// Map a foreign-key violation to [`Error::NotFound`] of the kind `kinds`
/// pairs with its constraint name (each module lists the exact generated
/// names of its own composite `(org_id, x_id)` keys, which refuse another
/// org's id exactly as they refuse a missing one). A violation of a
/// constraint not listed, or any other failure, is an internal
/// [`Error::Database`], never a guessed kind. The constraint name is never
/// shown.
pub(crate) fn map_fk(e: sqlx::Error, kinds: &[(&str, &'static str)]) -> Error {
    let kind = match &e {
        sqlx::Error::Database(db) if db.is_foreign_key_violation() => db
            .constraint()
            .and_then(|c| kinds.iter().find(|(name, _)| *name == c))
            .map(|(_, kind)| *kind),
        _ => None,
    };
    match kind {
        Some(kind) => Error::NotFound { kind },
        None => Error::Database(e),
    }
}

/// A fake database error for unit tests: sqlx's own Postgres error cannot be
/// built outside sqlx.
#[cfg(test)]
pub(crate) mod fake {
    use std::borrow::Cow;

    pub(crate) const DB_TEXT: &str =
        "duplicate key value violates unique constraint \"customers_org_id_email_key\"";

    #[derive(Debug)]
    pub(crate) struct FakeDbError {
        code: &'static str,
        constraint: Option<&'static str>,
    }

    impl std::fmt::Display for FakeDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(DB_TEXT)
        }
    }

    impl std::error::Error for FakeDbError {}

    impl sqlx::error::DatabaseError for FakeDbError {
        fn message(&self) -> &str {
            DB_TEXT
        }
        fn code(&self) -> Option<Cow<'_, str>> {
            Some(Cow::Borrowed(self.code))
        }
        fn constraint(&self) -> Option<&str> {
            self.constraint
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
            match self.code {
                "23505" => sqlx::error::ErrorKind::UniqueViolation,
                "23503" => sqlx::error::ErrorKind::ForeignKeyViolation,
                _ => sqlx::error::ErrorKind::Other,
            }
        }
    }

    /// A database error with SQLSTATE `code` and no constraint.
    pub(crate) fn db(code: &'static str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(FakeDbError {
            code,
            constraint: None,
        }))
    }

    /// A foreign-key violation (`23503`) naming `constraint`.
    pub(crate) fn fk(constraint: Option<&'static str>) -> sqlx::Error {
        sqlx::Error::Database(Box::new(FakeDbError {
            code: "23503",
            constraint,
        }))
    }

    /// Every pair in a module's constraint table maps to its kind.
    pub(crate) fn assert_kinds(kinds: &[(&'static str, &'static str)]) {
        assert!(!kinds.is_empty());
        for (name, kind) in kinds {
            let e = super::map_fk(fk(Some(*name)), kinds);
            assert!(
                matches!(&e, super::Error::NotFound { kind: k } if k == kind),
                "{name}: {e:?}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::db;
    use super::*;

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
                Error::Platform(otto_resource::Error::Status {
                    status: 503,
                    body: String::new(),
                }),
                "unavailable",
            ),
            (
                Error::Platform(otto_resource::Error::NotFound),
                "access_revoked",
            ),
            (
                Error::Platform(otto_resource::Error::Unauthorized),
                "internal",
            ),
            (
                Error::Platform(otto_resource::Error::Decode("x".into())),
                "internal",
            ),
            (
                Error::Tenant(otto_tenant::Error::Db(db("23505"))),
                "internal",
            ),
            (Error::Database(db("23505")), "internal"),
            (Error::Internal("bug".into()), "internal"),
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
    fn constraint_violations_map_to_domain_errors() {
        let e = on_unique(db("23505"), || Error::Conflict("dup".into()));
        assert_eq!(e.code(), "conflict");
        let e = on_unique(db("23503"), || Error::Conflict("dup".into()));
        assert_eq!(e.code(), "internal");

        let kinds = &[("tickets_org_id_customer_id_fkey", "customer")];
        let e = map_fk(
            super::fake::fk(Some("tickets_org_id_customer_id_fkey")),
            kinds,
        );
        assert!(matches!(e, Error::NotFound { kind: "customer" }), "{e:?}");
        // A constraint the table does not list, none at all, another
        // violation, or no database error: internal, never a guessed kind.
        for e in [
            super::fake::fk(Some("some_other_fkey")),
            super::fake::fk(None),
            db("23505"),
            sqlx::Error::PoolClosed,
        ] {
            let e = map_fk(e, kinds);
            assert!(matches!(e, Error::Database(_)), "{e:?}");
            assert_eq!(e.code(), "internal");
        }
    }

    /// A database error reads and codes the same whether it arrived directly
    /// or through the tenant substrate.
    #[test]
    fn tenant_database_errors_are_database_errors() {
        for code in ["22001", "23514", "23505", "40001"] {
            let direct = Error::Database(db(code));
            let tenant = Error::Tenant(otto_tenant::Error::Db(db(code)));
            assert_eq!(direct.code(), tenant.code(), "{code}");
            assert_eq!(direct.retriable(), tenant.retriable(), "{code}");
            assert_eq!(direct.is_internal(), tenant.is_internal(), "{code}");
            assert_eq!(direct.to_string(), tenant.to_string(), "{code}");
        }
        assert_eq!(
            Error::Tenant(otto_tenant::Error::Db(db("22001"))).code(),
            "invalid_argument"
        );
    }

    #[test]
    fn internal_errors_hide_their_detail_but_log_it() {
        let e = Error::Internal("metered a free tool".into());
        assert_eq!(e.code(), "internal");
        assert!(e.is_internal());
        assert!(!e.retriable());
        assert_eq!(e.to_string(), INTERNAL_MESSAGE);
        assert!(
            e.log_detail().contains("metered a free tool"),
            "{}",
            e.log_detail()
        );
    }

    /// Only a failure that can pass tells the caller to retry; a platform
    /// that no longer knows the org is a revocation; anything else is ours.
    #[test]
    fn platform_failures_split_by_whether_a_retry_can_help() {
        let down = Error::Platform(otto_resource::Error::Status {
            status: 502,
            body: "bad gateway".into(),
        });
        assert_eq!(down.code(), "unavailable");
        assert!(down.retriable() && down.is_internal());
        assert!(down.to_string().contains("Retry shortly"), "{down}");

        let gone = Error::Platform(otto_resource::Error::NotFound);
        assert_eq!(gone.code(), "access_revoked");
        assert!(!gone.retriable() && !gone.is_internal());
        assert_eq!(gone.to_string(), Error::AccessRevoked.to_string());

        for broken in [
            otto_resource::Error::Unauthorized,
            otto_resource::Error::Status {
                status: 404,
                body: "<html>".into(),
            },
            otto_resource::Error::Decode("x".into()),
        ] {
            let e = Error::Platform(broken);
            assert_eq!(e.code(), "internal", "{e:?}");
            assert!(!e.retriable() && e.is_internal(), "{e:?}");
            assert_eq!(e.to_string(), INTERNAL_MESSAGE);
            assert!(
                e.log_detail().starts_with("platform: "),
                "{}",
                e.log_detail()
            );
        }
    }
}
