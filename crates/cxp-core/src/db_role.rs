//! Who this service connects to Postgres as, whether that role can step
//! around row-level security or out of its own database, and whether the
//! instance is shared with the other otto services.
//!
//! Tenant transactions drop to `cxp_app` ([`crate::TENANT_ROLE`]) and are proven isolated at boot,
//! but everything else (migrations, the maintenance sweep and usage shipper,
//! the `org.deleted` tombstone, `/readyz`) runs as the connecting role
//! itself. The unpinned policies bind that role only while it is neither a
//! superuser nor BYPASSRLS, and on the shared `otto-db` a superuser also
//! reaches every other service's database. `cxp-server` reads this at boot
//! and refuses a privileged role, or one that can connect to another
//! service's database, in a deployment (#16).
//!
//! Two users are inspected. `current_user` is who statements run as;
//! `session_user` is who logged in, and a session can always `RESET ROLE`
//! back to it, so a superuser login whose connections start with
//! `SET ROLE cxp_server` (`?options=-c role=...`, or `ALTER ROLE ... SET
//! role`) is still a superuser connection.
//!
//! "Privileged" is anything superuser-equivalent or that reaches past this
//! service: SUPERUSER, BYPASSRLS, CREATEROLE (before Postgres 16 it can grant
//! itself any non-superuser role, BYPASSRLS ones included), CREATEDB (new
//! databases on a shared instance), REPLICATION, and the predefined roles in
//! [`PREDEFINED_ROLES`]. None of the attributes is inherited through
//! membership, but a member of a role can `SET ROLE` to it (a Postgres 16
//! `GRANT ... WITH INHERIT FALSE, SET TRUE` included), so membership counts
//! too. `pg_database_owner` is not in the list: every database owner is
//! implicitly a member of it, and it holds nothing beyond that database.
//!
//! Ownership is the other way around row-level security (#27). An owner
//! may `ALTER TABLE ... NO FORCE ROW LEVEL SECURITY`, drop a policy, or drop
//! the table; the owner of a database or schema may drop what is in it; an
//! extension's owner may drop it with every object it holds. `TRUNCATE`
//! ignores RLS outright, and `TRIGGER` lets a role attach code that runs as
//! whoever next writes the table. Postgres 17's `MAINTAIN` (`LOCK`,
//! `CLUSTER`, `REINDEX`, ...) does not get around RLS, but would let the
//! runtime role cause denial of service on the shared instance, so it counts
//! too. So the check also reads, per user, the objects in the current
//! database owned by a role it is or can `SET ROLE` to ([`DbRole::owned`]),
//! and the tables it may `TRUNCATE`, add a `TRIGGER` to, or `MAINTAIN`
//! without owning them ([`DbRole::table_privileges`]). The runtime role must
//! have none of them; the migration owner has the first by design. Schema
//! `public` is owned by `pg_database_owner`, so the database's owner is
//! reported as owning it.
//!
//! Creating objects is the last way around it ([`DbRole::create_privileges`]).
//! `TEMPORARY` on the database lets a role create a temporary table named
//! like a tenant table: `pg_temp` is searched before every other schema for
//! relations, so unqualified statements later run on that pooled connection,
//! tenant transactions included, would read and write a table with no RLS,
//! and a temporary table lives as long as the connection, not the
//! transaction. `CREATE` on a schema lets a role plant an exact-match
//! overload of a function a `SECURITY DEFINER` function calls, which then
//! runs as that function's owner (the CVE-2018-1058 pattern; migration 0007
//! also schema-qualifies those calls and pins `pg_temp` last). `CREATE` on
//! the database creates new schemas. So the check reads, per user and for
//! every role it is a member of (inherited or not), `CREATE` and `TEMPORARY`
//! on the current database and `CREATE` on each non-system schema, skipping
//! a database or schema the user owns through membership (reported as owned
//! already). `PUBLIC` holds `TEMPORARY` on every database by default; the
//! runbook revokes it.
//!
//! Not every kind of object is listed as owned: operators, operator classes
//! and families, collations, conversions, casts, text-search objects,
//! foreign-data wrappers and servers, publications, and event triggers are
//! not (event triggers need a superuser anyway). None of them gets around
//! row-level security on its own; the kinds that do (relations, the database,
//! schemas, extensions, functions, and types) are all listed.

use otto_tenant::Db;

use crate::Result;

/// At most this many privileged roles, reachable databases, owned objects,
/// tables it may `TRUNCATE`, add a `TRIGGER` to, or `MAINTAIN`, or places it
/// may create in are read per user: one is enough to refuse. Public only so
/// tests can follow it.
#[doc(hidden)]
pub const MAX_REPORTED: i64 = 8;

/// The predefined roles whose members reach past this service: server files
/// and programs, every table in every database they can open, other
/// sessions (signals, statistics, monitoring), subscriptions, and
/// maintenance of every relation. Looked up by name, so one a server version
/// lacks (`pg_create_subscription` before 16, `pg_maintain` before 17) is
/// simply not found.
pub const PREDEFINED_ROLES: [&str; 10] = [
    "pg_execute_server_program",
    "pg_read_server_files",
    "pg_write_server_files",
    "pg_read_all_data",
    "pg_write_all_data",
    "pg_signal_backend",
    "pg_read_all_stats",
    "pg_monitor",
    "pg_create_subscription",
    "pg_maintain",
];

/// The other otto services' databases on `otto-db`. Their presence on the
/// instance means it is shared with them, which makes this process a
/// deployment whatever its URLs say (a local run through `fly proxy` looks
/// like loopback). `CONNECT` to one of them counts however it is held,
/// `PUBLIC` included: `otto_app`, which this service's login may still
/// belong to (until runbook T5), holds table privileges there.
///
/// `otto_cxp` itself is not in the list: the test cluster's `otto_cxp`
/// database sits beside the per-test databases `#[sqlx::test]` creates, and
/// would make every test look shared.
pub const OTHER_SERVICE_DATABASES: [&str; 3] = ["otto_platform", "otto_factory", "otto_flags"];

/// The schemas whose objects count for ownership and table privileges:
/// every one but the system catalogs, TOAST, and temporary schemas.
const SCHEMAS: &str = "SELECT n.oid, n.nspname, n.nspowner FROM pg_catalog.pg_namespace n \
     WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
       AND n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp%'";

/// Databases a connection may always open: Postgres's own maintenance
/// database and Fly's `repmgr` (no service data in either).
const ALWAYS_REACHABLE: [&str; 2] = ["postgres", "repmgr"];

/// The connecting session, every privileged role it is or can become, the
/// other databases it can open, and the other services sharing its instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbRole {
    /// `session_user`: who logged in, and who `RESET ROLE` returns to.
    pub session_user: String,
    /// `current_user`: who statements run as. Differs from `session_user`
    /// only when the connection starts with a `SET ROLE`.
    pub current_user: String,
    /// The privileged roles either user is or is a member of: the session
    /// user's first, each user's own entry before its memberships. Empty for
    /// a least-privilege login; only itself for a superuser (which counts as
    /// a member of every role).
    pub privileged: Vec<PrivilegedRole>,
    /// Other databases either user can `CONNECT` to ([`OTHER_SERVICE_DATABASES`]
    /// through any grant, `PUBLIC` included; any other database through a
    /// grant or ownership that is not `PUBLIC`'s), besides the current one,
    /// templates, `postgres`, and `repmgr`. Not read for a superuser user,
    /// which may connect to every database and is refused already.
    pub other_databases: Vec<DatabaseAccess>,
    /// The [`OTHER_SERVICE_DATABASES`] that exist on this instance.
    pub shared_with: Vec<String>,
    /// Objects in the current database owned by a role either user is or
    /// is a member of, at most `MAX_REPORTED` per user, by kind then
    /// name. Not read for a superuser user, which owns everything in effect
    /// and is refused already.
    pub owned: Vec<OwnedObject>,
    /// How many objects the users own in all (each user's count, summed),
    /// of which [`DbRole::owned`] lists the first few.
    pub owned_total: i64,
    /// Tables, views, and foreign tables either user, or a role it can `SET
    /// ROLE` to (inherited or not), may `TRUNCATE`, add a `TRIGGER` to, or
    /// (Postgres 17 and later) `MAINTAIN` without owning them, at most
    /// `MAX_REPORTED` per user. Not read for a superuser user.
    pub table_privileges: Vec<TablePrivilege>,
    /// Whether the check read the `MAINTAIN` privilege, so the summary's
    /// clean clause names it: true on Postgres 17 and later.
    pub reports_maintain: bool,
    /// The current database (`CREATE`, `TEMPORARY`) and schemas (`CREATE`)
    /// either user, or a role it can `SET ROLE` to (inherited or not), may
    /// create in without owning them, `PUBLIC`'s grants included: the
    /// database first, then schemas by name, at most `MAX_REPORTED` per user.
    /// Not read for a superuser user.
    pub create_privileges: Vec<CreatePrivilege>,
}

/// A place one of the session's users may create objects or temporary
/// tables in without owning it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePrivilege {
    /// The `session_user` or `current_user` that holds the privileges.
    pub user: String,
    /// `"database"` or `"schema"`.
    pub kind: &'static str,
    /// The database's or schema's name.
    pub name: String,
    /// `"CREATE"`, `"TEMPORARY"` (a database only), or both, in that order.
    pub privileges: Vec<&'static str>,
}

impl CreatePrivilege {
    /// `database "x"` or `schema "public"`, as the summary names it.
    pub fn object(&self) -> String {
        format!("{} {:?}", self.kind, self.name)
    }
}

/// One object in the current database that one of the session's users owns,
/// directly or through membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedObject {
    /// The `session_user` or `current_user` that is, or is a member of, the
    /// owner.
    pub user: String,
    /// One of [`OWNED_KINDS`]: `"database"`, `"schema"`, `"table"`, ...
    pub kind: &'static str,
    /// The object's name: schema-qualified when not on the default search
    /// path, with its argument types for a function.
    pub name: String,
    /// The owning role: `user` itself, or a role it can `SET ROLE` to
    /// (`pg_database_owner` for schema `public`).
    pub owner: String,
}

/// A relation one of the session's users may `TRUNCATE`, add a `TRIGGER` to,
/// or `MAINTAIN` (Postgres 17 and later) without owning it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePrivilege {
    /// The `session_user` or `current_user` that holds the privileges.
    pub user: String,
    pub table: String,
    /// `"TRUNCATE"`, `"TRIGGER"`, and `"MAINTAIN"` (Postgres 17 and later,
    /// the one the check reads), in that order, omitting the ones not held.
    pub privileges: Vec<&'static str>,
}

/// The ownership clause of [`DbRole::summary`] for a role that can change or
/// shadow nothing, on servers before Postgres 17 (no `MAINTAIN` privilege to
/// report).
pub const CLEAN_OWNERSHIP: &str = "it owns nothing in this database, can TRUNCATE or add a \
    TRIGGER to no table, and can create no schema, object, or temporary table in it";

/// [`CLEAN_OWNERSHIP`] on Postgres 17 and later, where the check also reads
/// `MAINTAIN`: a role that holds it can `LOCK` and `REINDEX` the shared
/// instance's tables, which is denial of service rather than an RLS bypass, so
/// a deployment refuses it too.
pub const CLEAN_OWNERSHIP_WITH_MAINTAIN: &str = "it owns nothing in this database, can TRUNCATE, \
    add a TRIGGER, or MAINTAIN no table, and can create no schema, object, or temporary table in \
    it";

/// The Postgres major version that added the `MAINTAIN` table privilege
/// (`LOCK`, `CLUSTER`, `REINDEX`, `VACUUM`, `ANALYZE`, `REFRESH MATERIALIZED
/// VIEW`). Looked up by name, so a smaller server simply never reports it.
const MAINTAIN_VERSION: i32 = 170000;

/// The kinds of owned object reported, in the order they are listed.
pub const OWNED_KINDS: [&str; 15] = [
    "database",
    "schema",
    "extension",
    "table",
    "partitioned table",
    "view",
    "materialized view",
    "foreign table",
    "sequence",
    "function",
    "procedure",
    "aggregate",
    "type",
    "composite type",
    "index",
];

/// What the runtime role needs besides the tenant role's privileges:
///
/// - `INSERT on deleted_orgs`: what it does as itself and a tenant transaction may not, on a
///   table it does not own (#27): the `org.deleted` tombstone. Migration 0006 grants it to
///   `cxp_runtime`.
/// - `inheriting cxp_app` ([`crate::TENANT_ROLE`]): the maintenance sweep, the usage shipper, and
///   the console lookup run as the connecting role and use the table and function grants it
///   inherits from the tenant role (#28); a `NOINHERIT` login, or a grant `WITH INHERIT FALSE`,
///   holds none of them, and otto-tenant's assumability probe (`pg_has_role(..., 'USAGE')`) is then
///   false and it skips `SET LOCAL ROLE`.
pub const RUNTIME_GRANTS: [&str; 2] = ["INSERT on deleted_orgs", "inheriting cxp_app"];

/// The tenant role the otto services on a shared instance may share. otto-cxp no longer uses it
/// (#28), but the release before it does, so the runtime login stays a member until that
/// release cannot come back (docs/deploy/fly.md, runbook T5).
pub const SHARED_TENANT_ROLE: &str = otto_tenant::db::DEFAULT_TENANT_ROLE;

/// One privileged role, and which of the session's users holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegedRole {
    /// The `session_user` or `current_user` that is, or is a member of, it.
    pub user: String,
    /// The privileged role: `user` itself, or a role it can `SET ROLE` to.
    pub name: String,
    pub privileges: Vec<Privilege>,
}

/// Another database one of the session's users can connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseAccess {
    /// The `session_user` or `current_user` that can connect.
    pub user: String,
    pub database: String,
    /// The grantee of the `CONNECT` it holds there: a role it is or belongs
    /// to (the owner's own entry included), or `PUBLIC`.
    pub through: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    Superuser,
    BypassRls,
    CreateRole,
    CreateDb,
    Replication,
    /// One of [`PREDEFINED_ROLES`]; the role's name says which.
    PredefinedRole,
}

impl Privilege {
    fn label(self) -> &'static str {
        match self {
            Privilege::Superuser => "SUPERUSER",
            Privilege::BypassRls => "BYPASSRLS",
            Privilege::CreateRole => "CREATEROLE",
            Privilege::CreateDb => "CREATEDB",
            Privilege::Replication => "REPLICATION",
            Privilege::PredefinedRole => "cross-service predefined role",
        }
    }
}

impl PrivilegedRole {
    fn attributes(&self) -> String {
        self.privileges
            .iter()
            .map(|p| p.label())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl DbRole {
    /// Whether the session is, or can `SET ROLE` to, a superuser-equivalent
    /// role.
    pub fn is_privileged(&self) -> bool {
        !self.privileged.is_empty()
    }

    /// Whether the session can connect to another service's database.
    pub fn reaches_other_databases(&self) -> bool {
        !self.other_databases.is_empty()
    }

    /// Whether a deployment must refuse this session.
    pub fn is_refused_in_a_deployment(&self) -> bool {
        self.is_privileged() || self.reaches_other_databases()
    }

    /// Whether the instance also hosts another otto service's database.
    pub fn shares_the_instance(&self) -> bool {
        !self.shared_with.is_empty()
    }

    /// Whether either user owns, or can `SET ROLE` to the owner of, an
    /// object in the current database.
    pub fn owns_objects(&self) -> bool {
        !self.owned.is_empty()
    }

    /// Whether either user can change or shadow the schema, and so step
    /// around row-level security, without a superuser-equivalent privilege:
    /// by owning something here, by holding `TRUNCATE`, `TRIGGER`, or (on
    /// Postgres 17 and later) `MAINTAIN` on a table, or by holding `CREATE` on
    /// the database or a schema or `TEMPORARY` on the database. `MAINTAIN`
    /// does not get around RLS, but would let the runtime role take locks and
    /// reindex the shared instance's tables.
    pub fn can_alter_tables(&self) -> bool {
        self.owns_objects()
            || !self.table_privileges.is_empty()
            || !self.create_privileges.is_empty()
    }

    /// Whether either user is itself a superuser, whose ownership and table
    /// privileges are not read.
    fn has_superuser_user(&self) -> bool {
        self.privileged
            .iter()
            .any(|p| p.name == p.user && p.privileges.contains(&Privilege::Superuser))
    }

    /// The clean ownership clause for this server: the one that names
    /// `MAINTAIN` on Postgres 17 and later, where the check read it.
    fn clean_ownership(&self) -> &'static str {
        if self.reports_maintain {
            CLEAN_OWNERSHIP_WITH_MAINTAIN
        } else {
            CLEAN_OWNERSHIP
        }
    }

    /// The ownership clause of [`DbRole::summary`], without its leading
    /// `"; "`, or `None` for a superuser session (nothing was read, so
    /// "owns nothing" would mislead).
    fn ownership_clause(&self, who: impl Fn(&str) -> String) -> Option<String> {
        if !self.can_alter_tables() {
            return (!self.has_superuser_user()).then(|| self.clean_ownership().to_string());
        }
        let same = self.session_user == self.current_user;
        let mut parts = Vec::new();
        if self.owns_objects() {
            let items: Vec<String> = self
                .owned
                .iter()
                .map(|o| {
                    let through = if o.owner == o.user {
                        String::new()
                    } else {
                        format!(" (through {:?})", o.owner)
                    };
                    let verb = if same { "" } else { "owns " };
                    format!("{}{verb}{} {:?}{through}", who(&o.user), o.kind, o.name)
                })
                .collect();
            let subject = if same { "it owns " } else { "" };
            parts.push(format!(
                "{subject}{} ({} objects in all)",
                items.join(", "),
                self.owned_total
            ));
        } else {
            parts.push("it owns nothing in this database".to_string());
        }
        parts.extend(self.table_privileges.iter().map(|t| {
            format!(
                "{}can {} table {:?}",
                who(&t.user),
                t.privileges.join(", "),
                t.table
            )
        }));
        parts.extend(self.create_privileges.iter().map(|c| {
            format!(
                "{}can {} in {}",
                who(&c.user),
                c.privileges.join(", "),
                c.object()
            )
        }));
        Some(parts.join("; "))
    }

    /// One line for the boot log, naming each privilege and each other
    /// database found, which user holds it, and the services sharing the
    /// instance.
    pub fn summary(&self) -> String {
        let same = self.session_user == self.current_user;
        let subject = if same {
            format!("database role {:?}", self.current_user)
        } else {
            format!(
                "database role {:?} (session user {:?})",
                self.current_user, self.session_user
            )
        };
        let who = |user: &str| {
            if same {
                String::new()
            } else if user == self.session_user {
                format!("session user {user:?} ")
            } else {
                format!("current user {user:?} ")
            }
        };
        let mut line = if !self.is_refused_in_a_deployment() {
            format!(
                "{subject} holds no superuser-equivalent privilege (SUPERUSER, BYPASSRLS, \
                 CREATEROLE, CREATEDB, REPLICATION, or a cross-service predefined role), \
                 directly or through membership, and can connect to no other service's database"
            )
        } else {
            let found: Vec<String> = self
                .privileged
                .iter()
                .map(|p| {
                    if p.name == p.user {
                        format!("{}is {}", who(&p.user), p.attributes())
                    } else {
                        format!(
                            "{}is a member of {:?} ({})",
                            who(&p.user),
                            p.name,
                            p.attributes()
                        )
                    }
                })
                .chain(self.other_databases.iter().map(|d| {
                    let through = if d.through == "PUBLIC" {
                        "PUBLIC".to_string()
                    } else {
                        format!("{:?}", d.through)
                    };
                    format!(
                        "{}can connect to database {:?} (through {through})",
                        who(&d.user),
                        d.database
                    )
                }))
                .collect();
            let sep = if same { " " } else { ": " };
            format!("{subject}{sep}{}", found.join("; "))
        };
        if let Some(clause) = self.ownership_clause(who) {
            line.push_str("; ");
            line.push_str(&clause);
        }
        if self.shares_the_instance() {
            line.push_str(&format!(
                "; the instance also hosts {}",
                self.shared_with
                    .iter()
                    .map(|d| format!("{d:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        line
    }

    /// A `DbRole` from literals, for tests: `(user, role, privileges)`, with
    /// no other database reachable, nothing owned or truncatable, and an
    /// unshared instance.
    #[doc(hidden)]
    pub fn for_test(
        session_user: &str,
        current_user: &str,
        privileged: &[(&str, &str, &[Privilege])],
    ) -> Self {
        Self {
            session_user: session_user.into(),
            current_user: current_user.into(),
            privileged: privileged
                .iter()
                .map(|&(user, name, privileges)| PrivilegedRole {
                    user: user.into(),
                    name: name.into(),
                    privileges: privileges.to_vec(),
                })
                .collect(),
            other_databases: Vec::new(),
            shared_with: Vec::new(),
            owned: Vec::new(),
            owned_total: 0,
            table_privileges: Vec::new(),
            reports_maintain: false,
            create_privileges: Vec::new(),
        }
    }
}

/// Read the connecting session's privileges, the other databases it can
/// open, and the otto services sharing its instance, from the catalog.
pub async fn inspect(db: &Db) -> Result<DbRole> {
    inspect_with(db, &OTHER_SERVICE_DATABASES).await
}

/// [`inspect`] with the other services' database names given, so a test can
/// stand in uniquely named databases for them without making the shared
/// test cluster look like `otto-db` to every other test running on it.
#[doc(hidden)]
pub async fn inspect_with(db: &Db, service_databases: &[&str]) -> Result<DbRole> {
    let mut conn = db.pool().acquire().await?;
    let (session_user, current_user): (String, String) =
        sqlx::query_as("SELECT session_user::text, current_user::text")
            .fetch_one(&mut *conn)
            .await?;
    // Postgres 17 added the `MAINTAIN` table privilege; older servers reject
    // `has_table_privilege(..., 'MAINTAIN')`, so the query names it only when
    // the server understands it.
    let server_version: i32 =
        sqlx::query_scalar("SELECT current_setting('server_version_num')::int")
            .fetch_one(&mut *conn)
            .await?;
    let reports_maintain = server_version >= MAINTAIN_VERSION;
    let mut users = vec![session_user.clone()];
    if current_user != session_user {
        users.push(current_user.clone());
    }
    let predefined: Vec<String> = PREDEFINED_ROLES.iter().map(|r| r.to_string()).collect();
    let services: Vec<String> = service_databases.iter().map(|d| d.to_string()).collect();
    let always: Vec<String> = ALWAYS_REACHABLE.iter().map(|d| d.to_string()).collect();
    let mut privileged = Vec::new();
    let mut other_databases = Vec::new();
    let mut owned = Vec::new();
    let mut owned_total = 0;
    let mut table_privileges = Vec::new();
    let mut create_privileges = Vec::new();
    for user in users {
        let rows: Vec<(String, bool, bool, bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT r.rolname::text, r.rolsuper, r.rolbypassrls, r.rolcreaterole, \
                    r.rolcreatedb, r.rolreplication, r.rolname::text = ANY($2::text[]) \
             FROM pg_catalog.pg_roles r \
             WHERE (r.rolsuper OR r.rolbypassrls OR r.rolcreaterole OR r.rolcreatedb \
                    OR r.rolreplication OR r.rolname::text = ANY($2::text[])) \
               AND pg_catalog.pg_has_role($1::text::name, r.oid, 'MEMBER') \
             ORDER BY r.rolname::text <> $1::text, r.rolname \
             LIMIT $3",
        )
        .bind(&user)
        .bind(&predefined)
        .bind(MAX_REPORTED)
        .fetch_all(&mut *conn)
        .await?;
        let mut found: Vec<PrivilegedRole> = rows
            .into_iter()
            .map(
                |(name, sup, bypass, createrole, createdb, replication, predefined)| {
                    let privileges = [
                        (sup, Privilege::Superuser),
                        (bypass, Privilege::BypassRls),
                        (createrole, Privilege::CreateRole),
                        (createdb, Privilege::CreateDb),
                        (replication, Privilege::Replication),
                        (predefined, Privilege::PredefinedRole),
                    ]
                    .into_iter()
                    .filter_map(|(has, p)| has.then_some(p))
                    .collect();
                    PrivilegedRole {
                        user: user.clone(),
                        name,
                        privileges,
                    }
                },
            )
            .collect();
        // A superuser is a member of every role and may connect to every
        // database; naming them adds nothing.
        let superuser = found
            .first()
            .is_some_and(|p| p.name == user && p.privileges.contains(&Privilege::Superuser));
        if superuser {
            found.truncate(1);
        }
        privileged.extend(found);
        if superuser {
            continue;
        }

        // Every database the user may CONNECT to, explained by the grant
        // that allows it (a role it holds the privileges of, before PUBLIC).
        // PUBLIC's default CONNECT counts only on the other services'
        // databases: it is on every database created without a revoke,
        // which on the test cluster means every per-test database.
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT d.datname::text, coalesce(g.through, 'PUBLIC') \
             FROM pg_catalog.pg_database d \
             LEFT JOIN LATERAL ( \
               SELECT CASE WHEN a.grantee = 0 THEN 'PUBLIC' \
                           ELSE pg_catalog.pg_get_userbyid(a.grantee)::text END AS through \
               FROM pg_catalog.aclexplode( \
                      coalesce(d.datacl, pg_catalog.acldefault('d', d.datdba))) a \
               WHERE a.privilege_type = 'CONNECT' \
                 AND (a.grantee = 0 \
                      OR pg_catalog.pg_has_role($1::text::name, a.grantee, 'USAGE')) \
               ORDER BY a.grantee = 0, a.grantee \
               LIMIT 1 \
             ) g ON true \
             WHERE d.datallowconn AND NOT d.datistemplate \
               AND d.datname <> pg_catalog.current_database() \
               AND NOT d.datname::text = ANY($3::text[]) \
               AND pg_catalog.has_database_privilege($1::text::name, d.oid, 'CONNECT') \
               AND (g.through IS DISTINCT FROM 'PUBLIC' OR d.datname::text = ANY($2::text[])) \
             ORDER BY d.datname \
             LIMIT $4",
        )
        .bind(&user)
        .bind(&services)
        .bind(&always)
        .bind(MAX_REPORTED)
        .fetch_all(&mut *conn)
        .await?;
        other_databases.extend(rows.into_iter().map(|(database, through)| DatabaseAccess {
            user: user.clone(),
            database,
            through,
        }));

        // The objects here owned by a role the user is or can SET ROLE to,
        // the first few by kind then name, and how many in all. Each kind is
        // a way around RLS for its owner: the database and its schemas (drop
        // or replace what is in them), extensions (dropped with every object
        // in them), tables and the rest of pg_class (NO FORCE RLS, drop a
        // policy, ALTER or drop the relation), and functions and types
        // (replace the code and types the triggers, policies, and the
        // console lookup run).
        let rows: Vec<(String, String, String, i64)> = sqlx::query_as(&format!(
            "WITH schemas AS ({SCHEMAS}), \
             objects AS ( \
               SELECT 'database' AS kind, d.datname::text AS name, d.datdba AS owner \
               FROM pg_catalog.pg_database d \
               WHERE d.datname = pg_catalog.current_database() \
               UNION ALL \
               SELECT 'schema', s.nspname::text, s.nspowner FROM schemas s \
               UNION ALL \
               SELECT 'extension', e.extname::text, e.extowner \
               FROM pg_catalog.pg_extension e \
               UNION ALL \
               SELECT CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'partitioned table' \
                        WHEN 'v' THEN 'view' WHEN 'm' THEN 'materialized view' \
                        WHEN 'S' THEN 'sequence' WHEN 'f' THEN 'foreign table' \
                        WHEN 'c' THEN 'composite type' ELSE 'index' END, \
                      c.oid::pg_catalog.regclass::text, c.relowner \
               FROM pg_catalog.pg_class c JOIN schemas s ON s.oid = c.relnamespace \
               WHERE c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f', 'c', 'i', 'I') \
               UNION ALL \
               SELECT CASE p.prokind WHEN 'p' THEN 'procedure' WHEN 'a' THEN 'aggregate' \
                        ELSE 'function' END, \
                      p.oid::pg_catalog.regprocedure::text, p.proowner \
               FROM pg_catalog.pg_proc p JOIN schemas s ON s.oid = p.pronamespace \
               UNION ALL \
               SELECT 'type', t.oid::pg_catalog.regtype::text, t.typowner \
               FROM pg_catalog.pg_type t JOIN schemas s ON s.oid = t.typnamespace \
               WHERE t.typrelid = 0 \
                 AND NOT EXISTS (SELECT 1 FROM pg_catalog.pg_type e WHERE e.typarray = t.oid) \
             ) \
             SELECT o.kind, o.name, pg_catalog.pg_get_userbyid(o.owner)::text, \
                    count(*) OVER () \
             FROM objects o \
             WHERE pg_catalog.pg_has_role($1::text::name, o.owner, 'MEMBER') \
             ORDER BY pg_catalog.array_position($2::text[], o.kind), o.name \
             LIMIT $3"
        ))
        .bind(&user)
        .bind(&OWNED_KINDS[..])
        .bind(MAX_REPORTED)
        .fetch_all(&mut *conn)
        .await?;
        owned_total += rows.first().map_or(0, |r| r.3);
        owned.extend(rows.into_iter().map(|(kind, name, owner, _)| {
            let known = OWNED_KINDS.into_iter().find(|k| *k == kind);
            // Unreachable: the query only produces OWNED_KINDS. Should the
            // two drift, the object is still reported, as "object", rather
            // than dropped from a list that decides whether to refuse.
            debug_assert!(known.is_some(), "kind {kind:?} is not in OWNED_KINDS");
            OwnedObject {
                user: user.clone(),
                kind: known.unwrap_or("object"),
                name,
                owner,
            }
        }));

        // Relations it may TRUNCATE (which ignores RLS), add a TRIGGER to
        // (code that runs as whoever writes next), or MAINTAIN (Postgres 17:
        // LOCK, CLUSTER, REINDEX, and the rest; denial of service, not an RLS
        // bypass) without owning them. has_table_privilege(user, ...) counts
        // only what the user inherits; a NOINHERIT login, or a grant WITH
        // INHERIT FALSE, SET TRUE, still reaches a role's privileges by SET
        // ROLE, so each role the user is a member of (itself included) is
        // asked in turn. The MAINTAIN expression is `false` before 17, where
        // the name would be rejected.
        let maintain_expr = if reports_maintain {
            "EXISTS (SELECT 1 FROM roles r WHERE \
             pg_catalog.has_table_privilege(r.oid, c.oid, 'MAINTAIN'))"
        } else {
            "false"
        };
        let rows: Vec<(String, bool, bool, bool)> = sqlx::query_as(&format!(
            "WITH schemas AS ({SCHEMAS}), \
             roles AS ( \
               SELECT r.oid FROM pg_catalog.pg_roles r \
               WHERE pg_catalog.pg_has_role($1::text::name, r.oid, 'MEMBER') \
             ), \
             relations AS ( \
               SELECT c.oid, \
                      EXISTS (SELECT 1 FROM roles r WHERE \
                              pg_catalog.has_table_privilege(r.oid, c.oid, 'TRUNCATE')) \
                        AS can_truncate, \
                      EXISTS (SELECT 1 FROM roles r WHERE \
                              pg_catalog.has_table_privilege(r.oid, c.oid, 'TRIGGER')) \
                        AS can_trigger, \
                      {maintain_expr} AS can_maintain \
               FROM pg_catalog.pg_class c JOIN schemas s ON s.oid = c.relnamespace \
               WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') \
                 AND NOT pg_catalog.pg_has_role($1::text::name, c.relowner, 'MEMBER') \
             ) \
             SELECT oid::pg_catalog.regclass::text, can_truncate, can_trigger, can_maintain \
             FROM relations WHERE can_truncate OR can_trigger OR can_maintain \
             ORDER BY 1 \
             LIMIT $2"
        ))
        .bind(&user)
        .bind(MAX_REPORTED)
        .fetch_all(&mut *conn)
        .await?;
        table_privileges.extend(
            rows.into_iter()
                .map(|(table, truncate, trigger, maintain)| TablePrivilege {
                    user: user.clone(),
                    table,
                    privileges: [
                        (truncate, "TRUNCATE"),
                        (trigger, "TRIGGER"),
                        (maintain, "MAINTAIN"),
                    ]
                    .into_iter()
                    .filter_map(|(has, p)| has.then_some(p))
                    .collect(),
                }),
        );

        // Where it may create without owning: CREATE and TEMPORARY on the
        // current database (a temporary table shadows a tenant table through
        // pg_temp), CREATE on a schema (an overload a SECURITY DEFINER
        // function resolves). As for TRUNCATE, each role the user is a
        // member of is asked, so a privilege one SET ROLE away counts;
        // has_*_privilege includes PUBLIC's grants.
        let rows: Vec<(String, String, bool, bool)> = sqlx::query_as(&format!(
            "WITH schemas AS ({SCHEMAS}), \
             roles AS ( \
               SELECT r.oid FROM pg_catalog.pg_roles r \
               WHERE pg_catalog.pg_has_role($1::text::name, r.oid, 'MEMBER') \
             ), \
             places AS ( \
               SELECT 0 AS ord, 'database' AS kind, d.datname::text AS name, \
                      EXISTS (SELECT 1 FROM roles r WHERE \
                              pg_catalog.has_database_privilege(r.oid, d.oid, 'CREATE')) \
                        AS can_create, \
                      EXISTS (SELECT 1 FROM roles r WHERE \
                              pg_catalog.has_database_privilege(r.oid, d.oid, 'TEMPORARY')) \
                        AS can_temp \
               FROM pg_catalog.pg_database d \
               WHERE d.datname = pg_catalog.current_database() \
                 AND NOT pg_catalog.pg_has_role($1::text::name, d.datdba, 'MEMBER') \
               UNION ALL \
               SELECT 1, 'schema', s.nspname::text, \
                      EXISTS (SELECT 1 FROM roles r WHERE \
                              pg_catalog.has_schema_privilege(r.oid, s.oid, 'CREATE')), \
                      false \
               FROM schemas s \
               WHERE NOT pg_catalog.pg_has_role($1::text::name, s.nspowner, 'MEMBER') \
             ) \
             SELECT kind, name, can_create, can_temp FROM places \
             WHERE can_create OR can_temp \
             ORDER BY ord, name \
             LIMIT $2"
        ))
        .bind(&user)
        .bind(MAX_REPORTED)
        .fetch_all(&mut *conn)
        .await?;
        create_privileges.extend(rows.into_iter().map(|(kind, name, create, temp)| {
            CreatePrivilege {
                user: user.clone(),
                kind: if kind == "database" {
                    "database"
                } else {
                    "schema"
                },
                name,
                privileges: [(create, "CREATE"), (temp, "TEMPORARY")]
                    .into_iter()
                    .filter_map(|(has, p)| has.then_some(p))
                    .collect(),
            }
        }));
    }
    let shared_with: Vec<String> = sqlx::query_scalar(
        "SELECT datname::text FROM pg_catalog.pg_database \
         WHERE datname::text = ANY($1::text[]) AND datname <> pg_catalog.current_database() \
         ORDER BY datname",
    )
    .bind(&services)
    .fetch_all(&mut *conn)
    .await?;
    Ok(DbRole {
        session_user,
        current_user,
        privileged,
        other_databases,
        shared_with,
        owned,
        owned_total,
        table_privileges,
        reports_maintain,
        create_privileges,
    })
}

/// The [`RUNTIME_GRANTS`] the connecting role lacks, in that order.
///
/// - Tombstone entry (`INSERT on deleted_orgs`): when `deleted_orgs` exists and `current_user`
///   may not insert into it; absent before migrations, when there is no table yet.
/// - Tenant entry (`inheriting cxp_app`): when `cxp_app` does not exist, or `current_user`
///   does not hold its privileges (`pg_has_role(..., 'USAGE')`).
pub async fn missing_runtime_grants(db: &Db) -> Result<Vec<&'static str>> {
    // has_table_privilege and pg_has_role are strict: a NULL oid (no table,
    // no role) gives NULL.
    let (no_insert, no_inherit): (bool, bool) = sqlx::query_as(
        "SELECT coalesce(NOT pg_catalog.has_table_privilege(current_user, \
                pg_catalog.to_regclass('public.deleted_orgs'), 'INSERT'), false), \
                coalesce(NOT pg_catalog.pg_has_role(current_user, \
                (SELECT oid FROM pg_catalog.pg_roles WHERE rolname = $1), 'USAGE'), \
                true)",
    )
    .bind(crate::TENANT_ROLE)
    .fetch_one(db.pool())
    .await?;
    Ok([no_insert, no_inherit]
        .into_iter()
        .zip(RUNTIME_GRANTS)
        .filter_map(|(missing, grant)| missing.then_some(grant))
        .collect())
}

/// Whether the session's login or current user is still a member of [`SHARED_TENANT_ROLE`]
/// (false where it does not exist). Boot warns in a deployment while it is: nothing in this
/// release uses it, and a grant another service makes to it would reach this login. True
/// for a superuser whenever the role exists (a member of every role), which a deployment
/// refuses anyway.
pub async fn holds_shared_tenant_role(db: &Db) -> Result<bool> {
    let member: bool = sqlx::query_scalar(
        "SELECT coalesce((SELECT pg_catalog.pg_has_role(session_user, r.oid, 'MEMBER') \
                              OR pg_catalog.pg_has_role(current_user, r.oid, 'MEMBER') \
                          FROM pg_catalog.pg_roles r WHERE r.rolname = $1), false)",
    )
    .bind(SHARED_TENANT_ROLE)
    .fetch_one(db.pool())
    .await?;
    Ok(member)
}

/// The runtime group role migration 0005 grants the tombstone `INSERT` to. A
/// deployment creates it (and grants it to its runtime login) before the first
/// migration; 0005 grants to it only when it already exists.
pub const CXP_RUNTIME_ROLE: &str = "cxp_runtime";

/// What migration 0005 needs to have left in place, in this order: the
/// [`CXP_RUNTIME_ROLE`] exists, and it holds `INSERT` on `deleted_orgs`.
///
/// 0005 grants the tombstone `INSERT` to `cxp_runtime` only if the role
/// already exists, and a migration never runs twice, so a deployment's
/// `cxp-server migrate` reads this *after* migrating and aborts the release
/// when either is missing: serving would refuse to boot on every machine
/// otherwise (`missing_runtime_grants`).
pub async fn missing_cxp_runtime_install(db: &Db) -> Result<Vec<&'static str>> {
    missing_cxp_runtime_install_named(db, CXP_RUNTIME_ROLE).await
}

/// [`missing_cxp_runtime_install`] with the role named, so a test can use a
/// role of its own that it creates and drops.
#[doc(hidden)]
pub async fn missing_cxp_runtime_install_named(db: &Db, role: &str) -> Result<Vec<&'static str>> {
    // has_table_privilege(role_name, ...) raises for a role that does not
    // exist, so it is asked only for a row of pg_roles (none -> no row ->
    // NULL -> coalesced false). to_regclass is NULL before migrations.
    let (exists, can_insert): (bool, bool) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = $1), \
                coalesce((SELECT pg_catalog.has_table_privilege($1, \
                          pg_catalog.to_regclass('public.deleted_orgs'), 'INSERT') \
                          FROM pg_catalog.pg_roles WHERE rolname = $1), false)",
    )
    .bind(role)
    .fetch_one(db.pool())
    .await?;
    Ok([
        (!exists, CXP_RUNTIME_MISSING_ROLE),
        (!can_insert, CXP_RUNTIME_MISSING_GRANT),
    ]
    .into_iter()
    .filter_map(|(missing, item)| missing.then_some(item))
    .collect())
}

/// The missing item when [`CXP_RUNTIME_ROLE`] does not exist.
pub const CXP_RUNTIME_MISSING_ROLE: &str = "the cxp_runtime role";
/// The missing item when `cxp_runtime` exists but cannot insert into
/// `deleted_orgs`.
pub const CXP_RUNTIME_MISSING_GRANT: &str = "INSERT on deleted_orgs for cxp_runtime";

#[cfg(test)]
mod tests {
    use super::Privilege::*;
    use super::*;

    #[test]
    fn the_inheritance_grant_names_the_tenant_role() {
        assert_eq!(
            RUNTIME_GRANTS[1],
            format!("inheriting {}", crate::TENANT_ROLE)
        );
    }

    #[test]
    fn the_summary_names_every_privilege_and_where_it_comes_from() {
        let plain = DbRole::for_test("cxp_server", "cxp_server", &[]);
        assert!(!plain.is_privileged());
        assert!(!plain.is_refused_in_a_deployment());
        assert_eq!(
            plain.summary(),
            "database role \"cxp_server\" holds no superuser-equivalent privilege (SUPERUSER, \
             BYPASSRLS, CREATEROLE, CREATEDB, REPLICATION, or a cross-service predefined role), \
             directly or through membership, and can connect to no other service's database; \
             it owns nothing in this database, can TRUNCATE or add a TRIGGER to no table, and can create no schema, object, or temporary table in it"
        );
        let sup = DbRole::for_test(
            "otto_cxp",
            "otto_cxp",
            &[("otto_cxp", "otto_cxp", &[Superuser, BypassRls])],
        );
        assert!(sup.is_privileged());
        assert_eq!(
            sup.summary(),
            r#"database role "otto_cxp" is SUPERUSER, BYPASSRLS"#
        );
        let member = DbRole::for_test(
            "cxp_server",
            "cxp_server",
            &[
                ("cxp_server", "cxp_server", &[CreateRole, CreateDb]),
                ("cxp_server", "pg_monitor", &[PredefinedRole]),
            ],
        );
        assert_eq!(
            member.summary(),
            r#"database role "cxp_server" is CREATEROLE, CREATEDB; is a member of "pg_monitor" (cross-service predefined role); it owns nothing in this database, can TRUNCATE or add a TRIGGER to no table, and can create no schema, object, or temporary table in it"#
        );
    }

    #[test]
    fn the_summary_names_the_session_user_when_it_differs() {
        let set_role = DbRole::for_test(
            "otto_cxp",
            "cxp_server",
            &[("otto_cxp", "otto_cxp", &[Superuser])],
        );
        assert!(set_role.is_privileged());
        assert_eq!(
            set_role.summary(),
            r#"database role "cxp_server" (session user "otto_cxp"): session user "otto_cxp" is SUPERUSER"#
        );
        let both = DbRole::for_test(
            "login",
            "cxp_server",
            &[
                ("login", "login", &[Replication]),
                ("cxp_server", "bypass", &[BypassRls]),
            ],
        );
        assert_eq!(
            both.summary(),
            r#"database role "cxp_server" (session user "login"): session user "login" is REPLICATION; current user "cxp_server" is a member of "bypass" (BYPASSRLS); it owns nothing in this database, can TRUNCATE or add a TRIGGER to no table, and can create no schema, object, or temporary table in it"#
        );
        let clean = DbRole::for_test("login", "cxp_server", &[]);
        assert!(clean
            .summary()
            .starts_with(r#"database role "cxp_server" (session user "login") holds no"#));
    }

    #[test]
    fn the_summary_names_other_databases_and_the_services_sharing_the_instance() {
        let mut reach = DbRole::for_test("cxp_server", "cxp_server", &[]);
        reach.other_databases = vec![
            DatabaseAccess {
                user: "cxp_server".into(),
                database: "otto_platform".into(),
                through: "otto_app".into(),
            },
            DatabaseAccess {
                user: "cxp_server".into(),
                database: "otto_flags".into(),
                through: "PUBLIC".into(),
            },
        ];
        reach.shared_with = vec!["otto_flags".into(), "otto_platform".into()];
        assert!(!reach.is_privileged());
        assert!(reach.reaches_other_databases());
        assert!(reach.is_refused_in_a_deployment());
        assert!(reach.shares_the_instance());
        assert_eq!(
            reach.summary(),
            r#"database role "cxp_server" can connect to database "otto_platform" (through "otto_app"); can connect to database "otto_flags" (through PUBLIC); it owns nothing in this database, can TRUNCATE or add a TRIGGER to no table, and can create no schema, object, or temporary table in it; the instance also hosts "otto_flags", "otto_platform""#
        );

        // Production after the runbook: clean, on a shared instance.
        let mut clean = DbRole::for_test("cxp_server", "cxp_server", &[]);
        clean.shared_with = vec!["otto_platform".into()];
        assert!(!clean.is_refused_in_a_deployment());
        assert!(clean.summary().ends_with(
            r#"and can connect to no other service's database; it owns nothing in this database, can TRUNCATE or add a TRIGGER to no table, and can create no schema, object, or temporary table in it; the instance also hosts "otto_platform""#
        ));
    }

    fn object(user: &str, kind: &'static str, name: &str, owner: &str) -> OwnedObject {
        OwnedObject {
            user: user.into(),
            kind,
            name: name.into(),
            owner: owner.into(),
        }
    }

    #[test]
    fn the_summary_names_what_the_role_owns_and_may_truncate() {
        let clean = DbRole::for_test("cxp_server", "cxp_server", &[]);
        assert!(!clean.owns_objects());
        assert!(!clean.can_alter_tables());
        assert!(clean.summary().ends_with(
            "; it owns nothing in this database, can TRUNCATE or add a TRIGGER to no table, and can create no schema, object, or temporary table in it"
        ));

        // The migration owner: owns the database, its schema, its tables.
        let mut owner = DbRole::for_test("cxp_server", "cxp_server", &[]);
        owner.owned = vec![
            object("cxp_server", "database", "otto_cxp", "cxp_server"),
            object("cxp_server", "schema", "public", "pg_database_owner"),
            object("cxp_server", "table", "customers", "cxp_server"),
        ];
        owner.owned_total = 42;
        owner.shared_with = vec!["otto_platform".into()];
        assert!(owner.owns_objects());
        assert!(owner.can_alter_tables());
        // Ownership is not, by itself, a superuser-equivalent privilege.
        assert!(!owner.is_refused_in_a_deployment());
        assert_eq!(
            owner.summary(),
            "database role \"cxp_server\" holds no superuser-equivalent privilege (SUPERUSER, \
             BYPASSRLS, CREATEROLE, CREATEDB, REPLICATION, or a cross-service predefined role), \
             directly or through membership, and can connect to no other service's database; \
             it owns database \"otto_cxp\", schema \"public\" (through \"pg_database_owner\"), \
             table \"customers\" (42 objects in all); the instance also hosts \"otto_platform\""
        );

        let mut truncate = DbRole::for_test("cxp_server", "cxp_server", &[]);
        truncate.table_privileges = vec![
            TablePrivilege {
                user: "cxp_server".into(),
                table: "customers".into(),
                privileges: vec!["TRUNCATE"],
            },
            TablePrivilege {
                user: "cxp_server".into(),
                table: "feedback".into(),
                privileges: vec!["TRUNCATE", "TRIGGER"],
            },
        ];
        assert!(!truncate.owns_objects());
        assert!(truncate.can_alter_tables());
        assert!(truncate.summary().ends_with(
            r#"; it owns nothing in this database; can TRUNCATE table "customers"; can TRUNCATE, TRIGGER table "feedback""#
        ));
    }

    #[test]
    fn the_summary_names_where_the_role_may_create() {
        let mut create = DbRole::for_test("cxp_server", "cxp_server", &[]);
        create.create_privileges = vec![
            CreatePrivilege {
                user: "cxp_server".into(),
                kind: "database",
                name: "otto_cxp".into(),
                privileges: vec!["CREATE", "TEMPORARY"],
            },
            CreatePrivilege {
                user: "cxp_server".into(),
                kind: "schema",
                name: "public".into(),
                privileges: vec!["CREATE"],
            },
        ];
        assert!(!create.owns_objects());
        assert!(create.can_alter_tables());
        assert!(!create.is_refused_in_a_deployment());
        assert!(create.summary().ends_with(
            r#"; it owns nothing in this database; can CREATE, TEMPORARY in database "otto_cxp"; can CREATE in schema "public""#
        ));

        let mut temp = DbRole::for_test("login", "cxp_server", &[]);
        temp.create_privileges = vec![CreatePrivilege {
            user: "login".into(),
            kind: "database",
            name: "otto_cxp".into(),
            privileges: vec!["TEMPORARY"],
        }];
        assert!(temp.can_alter_tables());
        assert!(temp.summary().ends_with(
            r#"; it owns nothing in this database; session user "login" can TEMPORARY in database "otto_cxp""#
        ));
    }

    #[test]
    fn the_ownership_clause_names_the_user_when_they_differ_and_skips_a_superuser() {
        let mut both = DbRole::for_test("login", "cxp_server", &[]);
        both.owned = vec![
            object("login", "table", "tickets", "login"),
            object("cxp_server", "table", "segments", "owners"),
        ];
        both.owned_total = 2;
        both.table_privileges = vec![TablePrivilege {
            user: "cxp_server".into(),
            table: "customers".into(),
            privileges: vec!["TRIGGER"],
        }];
        assert!(both.summary().ends_with(
            r#"; session user "login" owns table "tickets", current user "cxp_server" owns table "segments" (through "owners") (2 objects in all); current user "cxp_server" can TRIGGER table "customers""#
        ));

        // A superuser's ownership is not read: no "owns nothing" claim.
        let sup = DbRole::for_test(
            "otto_cxp",
            "cxp_server",
            &[("otto_cxp", "otto_cxp", &[Superuser])],
        );
        assert!(!sup.summary().contains("owns"), "{}", sup.summary());
    }

    #[test]
    fn a_postgres_17_role_reports_maintain_like_truncate() {
        // The clean clause names MAINTAIN only when the check read it (17+).
        let mut pg17 = DbRole::for_test("cxp_server", "cxp_server", &[]);
        assert!(!pg17.can_alter_tables());
        pg17.reports_maintain = true;
        assert!(pg17.summary().ends_with(CLEAN_OWNERSHIP_WITH_MAINTAIN));

        // A role that holds only MAINTAIN is refused, named like TRUNCATE.
        let mut maintain = DbRole::for_test("cxp_server", "cxp_server", &[]);
        maintain.reports_maintain = true;
        maintain.table_privileges = vec![TablePrivilege {
            user: "cxp_server".into(),
            table: "customers".into(),
            privileges: vec!["MAINTAIN"],
        }];
        assert!(maintain.can_alter_tables());
        assert!(maintain
            .summary()
            .ends_with(r#"; it owns nothing in this database; can MAINTAIN table "customers""#));
    }

    #[test]
    fn the_lists_hold_what_the_check_must_refuse_and_nothing_a_service_needs() {
        // Every database owner is implicitly a member of pg_database_owner.
        assert!(!PREDEFINED_ROLES.contains(&"pg_database_owner"));
        // This service's own database is never "another service's".
        assert!(!OTHER_SERVICE_DATABASES.contains(&"otto_cxp"));
        // MAINTAIN is Postgres 17's, and the version it landed in.
        assert_eq!(MAINTAIN_VERSION, 170000);
    }
}
