//! Who this service connects to Postgres as, and whether that role can step
//! around row-level security.
//!
//! Tenant transactions drop to `otto_app` and are proven isolated at boot,
//! but everything else (migrations, the maintenance sweep and usage shipper,
//! the `org.deleted` tombstone, `/readyz`) runs as the connecting role
//! itself. The unpinned policies bind that role only while it is neither a
//! superuser nor BYPASSRLS, and on the shared `otto-db` a superuser also
//! reaches every other service's database. `cxp-server` reads this at boot
//! and refuses a privileged role in production (#16).
//!
//! Neither attribute is inherited through membership, but a member of a role
//! can `SET ROLE` to it, so a member of a superuser role is one statement away
//! from being one. Membership therefore counts too.

use otto_tenant::Db;

use crate::Result;

/// At most this many privileged roles are read: one is enough to refuse.
const MAX_REPORTED: i64 = 8;

/// The connecting role and every privileged role it is, or can become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbRole {
    /// `current_user` of a fresh pool connection.
    pub name: String,
    /// Roles with SUPERUSER or BYPASSRLS that the connecting role is or is a
    /// member of, itself first. Empty for a least-privilege role; only
    /// itself for a superuser (which counts as a member of every role).
    pub privileged: Vec<PrivilegedRole>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivilegedRole {
    pub name: String,
    pub superuser: bool,
    pub bypassrls: bool,
}

impl PrivilegedRole {
    fn attributes(&self) -> &'static str {
        match (self.superuser, self.bypassrls) {
            (true, true) => "SUPERUSER, BYPASSRLS",
            (true, false) => "SUPERUSER",
            _ => "BYPASSRLS",
        }
    }
}

impl DbRole {
    /// Whether the connecting role is, or can `SET ROLE` to, a superuser or
    /// BYPASSRLS role.
    pub fn is_privileged(&self) -> bool {
        !self.privileged.is_empty()
    }

    /// One line for the boot log, naming each privilege found.
    pub fn summary(&self) -> String {
        if !self.is_privileged() {
            return format!(
                "database role {:?} is neither a superuser nor BYPASSRLS, nor a member of one",
                self.name
            );
        }
        let found: Vec<String> = self
            .privileged
            .iter()
            .map(|p| {
                if p.name == self.name {
                    format!("is {}", p.attributes())
                } else {
                    format!("is a member of {:?} ({})", p.name, p.attributes())
                }
            })
            .collect();
        format!("database role {:?} {}", self.name, found.join("; "))
    }
}

/// Read the connecting role's privileges from the catalog.
pub async fn inspect(db: &Db) -> Result<DbRole> {
    let mut conn = db.pool().acquire().await?;
    let name: String = sqlx::query_scalar("SELECT current_user::text")
        .fetch_one(&mut *conn)
        .await?;
    let rows: Vec<(String, bool, bool)> = sqlx::query_as(
        "SELECT r.rolname::text, r.rolsuper, r.rolbypassrls \
         FROM pg_catalog.pg_roles r \
         WHERE (r.rolsuper OR r.rolbypassrls) \
           AND pg_catalog.pg_has_role(current_user, r.oid, 'MEMBER') \
         ORDER BY r.rolname <> current_user, r.rolname \
         LIMIT $1",
    )
    .bind(MAX_REPORTED)
    .fetch_all(&mut *conn)
    .await?;
    let mut privileged: Vec<PrivilegedRole> = rows
        .into_iter()
        .map(|(name, superuser, bypassrls)| PrivilegedRole {
            name,
            superuser,
            bypassrls,
        })
        .collect();
    if privileged
        .first()
        .is_some_and(|p| p.name == name && p.superuser)
    {
        privileged.truncate(1);
    }
    Ok(DbRole { name, privileged })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(name: &str, privileged: &[(&str, bool, bool)]) -> DbRole {
        DbRole {
            name: name.into(),
            privileged: privileged
                .iter()
                .map(|&(n, s, b)| PrivilegedRole {
                    name: n.into(),
                    superuser: s,
                    bypassrls: b,
                })
                .collect(),
        }
    }

    #[test]
    fn the_summary_names_every_privilege_and_where_it_comes_from() {
        let plain = role("cxp_server", &[]);
        assert!(!plain.is_privileged());
        assert_eq!(
            plain.summary(),
            r#"database role "cxp_server" is neither a superuser nor BYPASSRLS, nor a member of one"#
        );
        let sup = role("otto_cxp", &[("otto_cxp", true, true)]);
        assert!(sup.is_privileged());
        assert_eq!(
            sup.summary(),
            r#"database role "otto_cxp" is SUPERUSER, BYPASSRLS"#
        );
        let member = role(
            "cxp_server",
            &[("cxp_server", false, true), ("postgres", true, false)],
        );
        assert_eq!(
            member.summary(),
            r#"database role "cxp_server" is BYPASSRLS; is a member of "postgres" (SUPERUSER)"#
        );
    }
}
