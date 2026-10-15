//! Who this service connects to Postgres as, whether that role can step
//! around row-level security or out of its own database, and whether the
//! instance is shared with the other otto services.
//!
//! Tenant transactions drop to `otto_app` and are proven isolated at boot,
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
//! whoever next writes the table. So the check also reads, per user, the
//! objects in the current database owned by a role it is or can `SET ROLE`
//! to ([`DbRole::owned`]), and the tables it may `TRUNCATE` or add a
//! `TRIGGER` to without owning them ([`DbRole::table_privileges`]). The
//! runtime role must have neither; the migration owner has the first by
//! design. Schema `public` is owned by `pg_database_owner`, so the
//! database's owner is reported as owning it.

use otto_tenant::Db;

use crate::Result;

/// At most this many privileged roles, reachable databases, owned objects,
/// or tables it may `TRUNCATE` or add a `TRIGGER` to are read per user: one
/// is enough to refuse.
const MAX_REPORTED: i64 = 8;

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
/// `PUBLIC` included: `otto_app`, which this service's role belongs to, holds
/// table privileges there.
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
    /// Tables, views, and foreign tables either user may `TRUNCATE` or add
    /// a `TRIGGER` to without owning them, at most `MAX_REPORTED` per
    /// user. Not read for a superuser user.
    pub table_privileges: Vec<TablePrivilege>,
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

/// A relation one of the session's users may `TRUNCATE` or add a `TRIGGER`
/// to without owning it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePrivilege {
    /// The `session_user` or `current_user` that holds the privileges.
    pub user: String,
    pub table: String,
    /// `"TRUNCATE"`, `"TRIGGER"`, or both, in that order.
    pub privileges: Vec<&'static str>,
}

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

/// What the runtime role needs beyond `otto_app`: what it does as itself
/// rather than as `otto_app`, on a table it no longer owns once the
/// migration owner is split from it (#27). Only the `org.deleted`
/// tombstone. Migration 0006 grants it to `cxp_runtime`.
pub const RUNTIME_GRANTS: [&str; 1] = ["INSERT on deleted_orgs"];

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

    /// Whether either user can step around row-level security on a table
    /// without a superuser-equivalent privilege: by owning something here,
    /// or by holding `TRUNCATE` or `TRIGGER` on a table.
    pub fn can_alter_tables(&self) -> bool {
        self.owns_objects() || !self.table_privileges.is_empty()
    }

    /// Whether either user is itself a superuser, whose ownership and table
    /// privileges are not read.
    fn has_superuser_user(&self) -> bool {
        self.privileged
            .iter()
            .any(|p| p.name == p.user && p.privileges.contains(&Privilege::Superuser))
    }

    /// The ownership clause of [`DbRole::summary`], without its leading
    /// `"; "`, or `None` for a superuser session (nothing was read, so
    /// "owns nothing" would mislead).
    fn ownership_clause(&self, who: impl Fn(&str) -> String) -> Option<String> {
        if !self.can_alter_tables() {
            return (!self.has_superuser_user()).then(|| {
                "it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table"
                    .to_string()
            });
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

        // Every object here owned by a role the user is or can SET ROLE to,
        // the first few by kind then name, and how many in all.
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
            OwnedObject {
                user: user.clone(),
                kind: OWNED_KINDS
                    .into_iter()
                    .find(|k| *k == kind)
                    .unwrap_or("object"),
                name,
                owner,
            }
        }));

        // Relations it may TRUNCATE (which ignores RLS) or add a TRIGGER to
        // (code that runs as whoever writes next) without owning them.
        let rows: Vec<(String, bool, bool)> = sqlx::query_as(&format!(
            "WITH schemas AS ({SCHEMAS}) \
             SELECT c.oid::pg_catalog.regclass::text, \
                    pg_catalog.has_table_privilege($1::text::name, c.oid, 'TRUNCATE'), \
                    pg_catalog.has_table_privilege($1::text::name, c.oid, 'TRIGGER') \
             FROM pg_catalog.pg_class c JOIN schemas s ON s.oid = c.relnamespace \
             WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') \
               AND NOT pg_catalog.pg_has_role($1::text::name, c.relowner, 'MEMBER') \
               AND (pg_catalog.has_table_privilege($1::text::name, c.oid, 'TRUNCATE') \
                    OR pg_catalog.has_table_privilege($1::text::name, c.oid, 'TRIGGER')) \
             ORDER BY 1 \
             LIMIT $2"
        ))
        .bind(&user)
        .bind(MAX_REPORTED)
        .fetch_all(&mut *conn)
        .await?;
        table_privileges.extend(rows.into_iter().map(|(table, truncate, trigger)| {
            TablePrivilege {
                user: user.clone(),
                table,
                privileges: [(truncate, "TRUNCATE"), (trigger, "TRIGGER")]
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
    })
}

/// The [`RUNTIME_GRANTS`] the connecting role lacks: `INSERT on
/// deleted_orgs` when that table exists and `current_user` may not insert
/// into it. Empty before migrations (no table yet) and for its owner.
pub async fn missing_runtime_grants(db: &Db) -> Result<Vec<&'static str>> {
    // has_table_privilege is strict: a NULL oid (no table) gives NULL.
    let missing: bool = sqlx::query_scalar(
        "SELECT coalesce(NOT pg_catalog.has_table_privilege(current_user, \
                pg_catalog.to_regclass('public.deleted_orgs'), 'INSERT'), false)",
    )
    .fetch_one(db.pool())
    .await?;
    Ok(if missing {
        RUNTIME_GRANTS.to_vec()
    } else {
        Vec::new()
    })
}

#[cfg(test)]
mod tests {
    use super::Privilege::*;
    use super::*;

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
             it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table"
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
            r#"database role "cxp_server" is CREATEROLE, CREATEDB; is a member of "pg_monitor" (cross-service predefined role); it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table"#
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
            r#"database role "cxp_server" (session user "login"): session user "login" is REPLICATION; current user "cxp_server" is a member of "bypass" (BYPASSRLS); it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table"#
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
            r#"database role "cxp_server" can connect to database "otto_platform" (through "otto_app"); can connect to database "otto_flags" (through PUBLIC); it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table; the instance also hosts "otto_flags", "otto_platform""#
        );

        // Production after the runbook: clean, on a shared instance.
        let mut clean = DbRole::for_test("cxp_server", "cxp_server", &[]);
        clean.shared_with = vec!["otto_platform".into()];
        assert!(!clean.is_refused_in_a_deployment());
        assert!(clean.summary().ends_with(
            r#"and can connect to no other service's database; it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table; the instance also hosts "otto_platform""#
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
            "; it owns nothing in this database and can TRUNCATE or add a TRIGGER to no table"
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
    fn the_lists_hold_what_the_check_must_refuse_and_nothing_a_service_needs() {
        // Every database owner is implicitly a member of pg_database_owner.
        assert!(!PREDEFINED_ROLES.contains(&"pg_database_owner"));
        // This service's own database is never "another service's".
        assert!(!OTHER_SERVICE_DATABASES.contains(&"otto_cxp"));
    }
}
