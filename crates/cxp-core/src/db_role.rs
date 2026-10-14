//! Who this service connects to Postgres as, and whether that role can step
//! around row-level security or out of its own database.
//!
//! Tenant transactions drop to `otto_app` and are proven isolated at boot,
//! but everything else (migrations, the maintenance sweep and usage shipper,
//! the `org.deleted` tombstone, `/readyz`) runs as the connecting role
//! itself. The unpinned policies bind that role only while it is neither a
//! superuser nor BYPASSRLS, and on the shared `otto-db` a superuser also
//! reaches every other service's database. `cxp-server` reads this at boot
//! and refuses a privileged role in a deployment (#16).
//!
//! Two users are inspected. `current_user` is who statements run as;
//! `session_user` is who logged in, and a session can always `RESET ROLE`
//! back to it, so a superuser login whose connections start with
//! `SET ROLE cxp_server` (`?options=-c role=...`, or `ALTER ROLE ... SET
//! role`) is still a superuser connection.
//!
//! "Privileged" is anything a superuser-equivalent: SUPERUSER, BYPASSRLS,
//! CREATEROLE (before Postgres 16 it can grant itself any non-superuser role,
//! BYPASSRLS ones included), REPLICATION, and the predefined roles that read
//! or write server files, run server programs, or read or write every table
//! ([`SERVER_ROLES`]). None of the attributes is inherited through
//! membership, but a member of a role can `SET ROLE` to it, so membership
//! counts too.

use otto_tenant::Db;

use crate::Result;

/// At most this many privileged roles are read per user: one is enough to
/// refuse.
const MAX_REPORTED: i64 = 8;

/// The predefined roles whose members reach past this database: server
/// files, server programs, or every table in every database they can open.
pub const SERVER_ROLES: [&str; 5] = [
    "pg_execute_server_program",
    "pg_read_server_files",
    "pg_write_server_files",
    "pg_read_all_data",
    "pg_write_all_data",
];

/// The connecting session and every privileged role it is, or can become.
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
}

/// One privileged role, and which of the session's users holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegedRole {
    /// The `session_user` or `current_user` that is, or is a member of, it.
    pub user: String,
    /// The privileged role: `user` itself, or a role it can `SET ROLE` to.
    pub name: String,
    pub privileges: Vec<Privilege>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    Superuser,
    BypassRls,
    CreateRole,
    Replication,
    /// One of [`SERVER_ROLES`]; the role's name says which.
    ServerRole,
}

impl Privilege {
    fn label(self) -> &'static str {
        match self {
            Privilege::Superuser => "SUPERUSER",
            Privilege::BypassRls => "BYPASSRLS",
            Privilege::CreateRole => "CREATEROLE",
            Privilege::Replication => "REPLICATION",
            Privilege::ServerRole => "server-access predefined role",
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

    /// One line for the boot log, naming each privilege found and which user
    /// holds it.
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
        if !self.is_privileged() {
            return format!(
                "{subject} holds no superuser-equivalent privilege (SUPERUSER, BYPASSRLS, \
                 CREATEROLE, REPLICATION, or a server-access predefined role), directly or \
                 through membership"
            );
        }
        let found: Vec<String> = self
            .privileged
            .iter()
            .map(|p| {
                let who = if same {
                    String::new()
                } else if p.user == self.session_user {
                    format!("session user {:?} ", p.user)
                } else {
                    format!("current user {:?} ", p.user)
                };
                if p.name == p.user {
                    format!("{who}is {}", p.attributes())
                } else {
                    format!("{who}is a member of {:?} ({})", p.name, p.attributes())
                }
            })
            .collect();
        if same {
            format!("{subject} {}", found.join("; "))
        } else {
            format!("{subject}: {}", found.join("; "))
        }
    }

    /// A `DbRole` from literals, for tests: `(user, role, privileges)`.
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
        }
    }
}

/// Read the connecting session's privileges from the catalog.
pub async fn inspect(db: &Db) -> Result<DbRole> {
    let mut conn = db.pool().acquire().await?;
    let (session_user, current_user): (String, String) =
        sqlx::query_as("SELECT session_user::text, current_user::text")
            .fetch_one(&mut *conn)
            .await?;
    let mut users = vec![session_user.clone()];
    if current_user != session_user {
        users.push(current_user.clone());
    }
    let server_roles: Vec<String> = SERVER_ROLES.iter().map(|r| r.to_string()).collect();
    let mut privileged = Vec::new();
    for user in users {
        let rows: Vec<(String, bool, bool, bool, bool, bool)> = sqlx::query_as(
            "SELECT r.rolname::text, r.rolsuper, r.rolbypassrls, r.rolcreaterole, \
                    r.rolreplication, r.rolname::text = ANY($2::text[]) \
             FROM pg_catalog.pg_roles r \
             WHERE (r.rolsuper OR r.rolbypassrls OR r.rolcreaterole OR r.rolreplication \
                    OR r.rolname::text = ANY($2::text[])) \
               AND pg_catalog.pg_has_role($1::text::name, r.oid, 'MEMBER') \
             ORDER BY r.rolname::text <> $1::text, r.rolname \
             LIMIT $3",
        )
        .bind(&user)
        .bind(&server_roles)
        .bind(MAX_REPORTED)
        .fetch_all(&mut *conn)
        .await?;
        let mut found: Vec<PrivilegedRole> = rows
            .into_iter()
            .map(|(name, sup, bypass, createrole, replication, server)| {
                let privileges = [
                    (sup, Privilege::Superuser),
                    (bypass, Privilege::BypassRls),
                    (createrole, Privilege::CreateRole),
                    (replication, Privilege::Replication),
                    (server, Privilege::ServerRole),
                ]
                .into_iter()
                .filter_map(|(has, p)| has.then_some(p))
                .collect();
                PrivilegedRole {
                    user: user.clone(),
                    name,
                    privileges,
                }
            })
            .collect();
        // A superuser is a member of every role; naming them adds nothing.
        if found
            .first()
            .is_some_and(|p| p.name == user && p.privileges.contains(&Privilege::Superuser))
        {
            found.truncate(1);
        }
        privileged.extend(found);
    }
    Ok(DbRole {
        session_user,
        current_user,
        privileged,
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
        assert_eq!(
            plain.summary(),
            "database role \"cxp_server\" holds no superuser-equivalent privilege (SUPERUSER, \
             BYPASSRLS, CREATEROLE, REPLICATION, or a server-access predefined role), directly \
             or through membership"
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
                ("cxp_server", "cxp_server", &[CreateRole]),
                ("cxp_server", "pg_read_all_data", &[ServerRole]),
            ],
        );
        assert_eq!(
            member.summary(),
            r#"database role "cxp_server" is CREATEROLE; is a member of "pg_read_all_data" (server-access predefined role)"#
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
            r#"database role "cxp_server" (session user "login"): session user "login" is REPLICATION; current user "cxp_server" is a member of "bypass" (BYPASSRLS)"#
        );
        let clean = DbRole::for_test("login", "cxp_server", &[]);
        assert!(clean
            .summary()
            .starts_with(r#"database role "cxp_server" (session user "login") holds no"#));
    }
}
