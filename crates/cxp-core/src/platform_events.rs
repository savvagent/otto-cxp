//! The otto platform's lifecycle events: `begin_live`, the
//! transaction every request path opens (refusing deleted orgs and removed
//! members), `apply` for the signed `org.deleted`, `member.removed`, and
//! `team.deleted` webhooks (idempotent on `event_id`), and the retention
//! `sweep` of the `platform_events` log.
//!
//! The platform owns orgs, teams, and memberships; this database holds rows
//! keyed by their ids with no foreign key to them. When the platform deletes
//! one it tells every registered resource server, and this module is
//! otto-cxp's cleanup. Deliveries are at-least-once and unordered, so
//! everything here is idempotent: each delivery is recorded in
//! `platform_events` in the same transaction as its effects, and each effect
//! is itself safe to repeat.
//!
//! | Event | Effect |
//! |---|---|
//! | `org.deleted` | Its tokens are refused (tombstone first), then every row this database holds for the org is deleted, in short transactions of at most [`PURGE_BATCH`] rows per table. Only the dedupe marker (written after the last batch), the tombstone, and usage the platform already refused (`usage_outbox_rejected`, swept after [`REJECTED_USAGE_KEEP_DAYS`]) survive. |
//! | `member.removed` | The user's cached token is refused for a few minutes, until the platform's own introspection stops vouching for it, and their console sessions in the org are deleted. What they created stays. |
//! | `team.deleted` | Catalog entities (experiences, applications, pages, components) owned by the team keep existing with `team_id` nulled. |
//!
//! # Why requests take a lock
//!
//! A request that passed the tombstone check a moment before an `org.deleted`
//! was applied could still re-create rows after the purge. So every request
//! transaction opens with [`begin_live`], which takes the org's lifecycle lock
//! *shared* and re-checks the tombstones while holding it, and the cleanup
//! takes the same lock *exclusively* after writing its tombstone. A
//! transaction already in flight finishes first and is cleaned up with
//! everything else; one that starts later sees the tombstone and is refused.

use std::time::Duration;

use otto_resource::webhook::{LifecycleEvent, WebhookEvent};
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::{Db, Tx};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Outcome {
    Applied {
        detail: serde_json::Value,
    },
    /// An earlier delivery already applied this event.
    Duplicate,
    /// An event this version does not act on. Acknowledged, not failed:
    /// failing it would only make the platform retry it forever.
    Ignored,
}

/// How long a removed member is refused. Past the introspection cache's 60 s
/// the platform itself answers "inactive"; this only has to outlast that.
pub const REMOVED_MEMBER_TTL_SECS: i64 = 300;

/// The advisory-lock class of an org's lifecycle lock ("cxlc"); the org
/// is the second key, as `hashtext(org::text)`.
pub const LIFECYCLE_LOCK: i32 = 0x6378_6c63;

/// Every table holding an org's domain rows, unshipped usage, or console
/// sessions, children before parents, as `org.deleted` purges them.
/// `platform_events` is kept (a replay must still find its dedupe marker) and
/// so is `usage_outbox_rejected`: it is the only record of usage that was
/// charged to the org but never counted by the platform, which billing
/// questions about the deleted org may still need, and the sweep forgets it
/// after [`REJECTED_USAGE_KEEP_DAYS`] anyway.
pub const PURGED_TABLES: &[&str] = &[
    "survey_responses",
    "surveys",
    "feedback",
    "chat_messages",
    "chat_sessions",
    "tickets",
    "knowledge_search_terms",
    "knowledge_articles",
    "segment_members",
    "segments",
    "interactions",
    "customer_search_grams",
    "customers",
    "components",
    "pages",
    "applications",
    "experiences",
    "usage_outbox",
    "console_sessions",
];

/// The most rows one `org.deleted` purge transaction deletes from any one
/// table. The purge runs as a series of short transactions, so a large org
/// neither holds its lifecycle lock (and every request to it) nor bloats one
/// transaction for the whole purge.
pub const PURGE_BATCH: i64 = 5000;

/// How long a `usage_outbox_rejected` row (usage the platform refused) is
/// kept for inspection before the sweep forgets it.
pub const REJECTED_USAGE_KEEP_DAYS: i32 = 90;

/// The catalog tables whose `team_id` a `team.deleted` nulls.
const TEAM_OWNED_TABLES: &[&str] = &["experiences", "applications", "pages", "components"];

/// Whether `user`'s token for `org` must be refused regardless of what the
/// platform's cached introspection says. An `Err` is a database failure and
/// must be answered `503`, not treated as "not revoked".
pub async fn revoked(db: &Db, org: OrgId, user: UserId) -> Result<bool> {
    let mut conn = db.pool().acquire().await?;
    revoked_on(&mut conn, org, Some(user)).await
}

async fn revoked_on(
    conn: &mut sqlx::PgConnection,
    org: OrgId,
    user: Option<UserId>,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM deleted_orgs WHERE org_id = $1) \
             OR EXISTS (SELECT 1 FROM removed_members \
                        WHERE org_id = $1 AND user_id = $2 \
                          AND removed_at > now() - make_interval(secs => $3))",
    )
    .bind(org)
    .bind(user)
    .bind(REMOVED_MEMBER_TTL_SECS as f64)
    .fetch_one(conn)
    .await?)
}

/// Open a transaction pinned to `org` that no lifecycle cleanup can
/// interleave with, refused with [`Error::AccessRevoked`] if the org was
/// deleted or `user` just removed. Every request path opens its transaction
/// here (or with [`begin_live_bounded`]), never with `Db::begin`.
pub async fn begin_live(db: &Db, org: OrgId, user: Option<UserId>) -> Result<Tx<'static>> {
    begin_live_inner(db, org, user, None).await
}

/// Database-side bounds on a transaction's waits: `lock_timeout` and
/// `statement_timeout`, both transaction-local.
///
/// Both are sent in whole milliseconds, at least 1: PostgreSQL reads `0` as
/// "no limit", so a bound under 1 ms is raised to 1 ms rather than turning
/// itself off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// The longest any one lock wait may last (`lock_timeout`).
    pub lock: Duration,
    /// The longest any one statement may run, waits included
    /// (`statement_timeout`).
    pub statement: Duration,
}

/// [`begin_live`], with `t` set for the rest of the transaction before it
/// waits on anything, the org's lifecycle lock included. A wait that runs
/// past either bound fails the statement, and so the transaction, instead of
/// waiting on. The settings are `SET LOCAL`: they end with the transaction
/// and never reach the next user of the connection.
pub async fn begin_live_bounded(
    db: &Db,
    org: OrgId,
    user: Option<UserId>,
    t: Timeouts,
) -> Result<Tx<'static>> {
    begin_live_inner(db, org, user, Some(t)).await
}

async fn begin_live_inner(
    db: &Db,
    org: OrgId,
    user: Option<UserId>,
    timeouts: Option<Timeouts>,
) -> Result<Tx<'static>> {
    let mut tx = db.begin(org).await?;
    if let Some(t) = timeouts {
        // `set_config` takes the setting's string form; a bare number is read
        // in the setting's unit, milliseconds. Never 0, which means no limit.
        // Every statement after this one counts against a caller's budget
        // (`console_sessions::STORE_STATEMENTS`): adding one here means
        // raising that count.
        let ms = |d: Duration| d.as_millis().max(1).to_string();
        sqlx::query(
            "SELECT set_config('lock_timeout', $1, true), \
                    set_config('statement_timeout', $2, true)",
        )
        .bind(ms(t.lock))
        .bind(ms(t.statement))
        .execute(tx.conn())
        .await?;
    }
    sqlx::query("SELECT pg_advisory_xact_lock_shared($1, hashtext($2::text))")
        .bind(LIFECYCLE_LOCK)
        .bind(org)
        .execute(tx.conn())
        .await?;
    if revoked_on(tx.conn(), org, user).await? {
        return Err(Error::AccessRevoked);
    }
    Ok(tx)
}

async fn exclude_requests(tx: &mut Tx<'_>, org: OrgId) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
        .bind(LIFECYCLE_LOCK)
        .bind(org)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// Housekeeping: forget dedupe markers older than `keep_days`, removed-member
/// tombstones past twice their [`REMOVED_MEMBER_TTL_SECS`] window, refused
/// usage older than [`REJECTED_USAGE_KEEP_DAYS`], and expired console
/// sessions. Unpinned, in maintenance transactions (`crate::maintenance`),
/// which the unpinned policies on `platform_events`, `usage_outbox_rejected`,
/// and `console_sessions` require.
///
/// Each step is its own transaction (the console sessions, several bounded
/// ones), so one failing neither rolls back nor stops the others. Returns how
/// many rows were deleted, or the first failure (each is also logged) once
/// every step has been tried.
pub async fn sweep(db: &Db, keep_days: i32) -> Result<u64> {
    let mut swept = 0;
    let mut failure = None;
    for step in [
        Sweep::InTx(TxSweep::Events),
        Sweep::InTx(TxSweep::Tombstones),
        Sweep::InTx(TxSweep::RejectedUsage),
        Sweep::ConsoleSessions,
    ] {
        let result = match step {
            Sweep::InTx(one) => sweep_in_tx(db, one, keep_days).await,
            // Its own bounded batches, each a maintenance transaction.
            Sweep::ConsoleSessions => crate::console_sessions::sweep_expired(db).await,
        };
        match result {
            Ok(n) => swept += n,
            Err(e) => {
                tracing::warn!(?step, error = %e.log_detail(), "a retention sweep step failed");
                failure.get_or_insert(e);
            }
        }
    }
    match failure {
        Some(e) => Err(e),
        None => Ok(swept),
    }
}

#[derive(Debug, Clone, Copy)]
enum Sweep {
    InTx(TxSweep),
    ConsoleSessions,
}

/// A sweep step that is one maintenance transaction.
#[derive(Debug, Clone, Copy)]
enum TxSweep {
    Events,
    Tombstones,
    RejectedUsage,
}

async fn sweep_in_tx(db: &Db, step: TxSweep, keep_days: i32) -> Result<u64> {
    let mut tx = crate::maintenance::begin(db).await?;
    let n = match step {
        TxSweep::Events => sqlx::query(
            "DELETE FROM platform_events WHERE received_at < now() - make_interval(days => $1)",
        )
        .bind(keep_days)
        .execute(&mut *tx)
        .await?
        .rows_affected(),
        // the tenant role may not DELETE tombstones; this database function deletes
        // only expired ones (twice REMOVED_MEMBER_TTL_SECS, fixed in the
        // migration).
        TxSweep::Tombstones => {
            let n: i64 = sqlx::query_scalar("SELECT expire_removed_members()")
                .fetch_one(&mut *tx)
                .await?;
            n as u64
        }
        TxSweep::RejectedUsage => sqlx::query(
            "DELETE FROM usage_outbox_rejected \
             WHERE rejected_at < now() - make_interval(days => $1)",
        )
        .bind(REJECTED_USAGE_KEEP_DAYS)
        .execute(&mut *tx)
        .await?
        .rows_affected(),
    };
    tx.commit().await?;
    Ok(n)
}

/// Apply one verified delivery.
pub async fn apply(db: &Db, event: &WebhookEvent) -> Result<Outcome> {
    match &event.event {
        LifecycleEvent::OrgDeleted { org_id } => org_deleted(db, event, (*org_id).into()).await,
        LifecycleEvent::MemberRemoved { org_id, user_id } => {
            member_removed(db, event, (*org_id).into(), (*user_id).into()).await
        }
        LifecycleEvent::TeamDeleted { org_id, team_id } => {
            team_deleted(db, event, (*org_id).into(), *team_id).await
        }
        // A kind otto-resource itself does not know: a platform newer than
        // the pinned client added it, and no release of this service could
        // act on it without a client bump first. Acknowledged, as
        // otto-resource documents, or the platform would retry it forever.
        LifecycleEvent::Unknown { kind } => {
            tracing::warn!(kind = %kind, event_id = %event.id, "ignoring unknown platform event");
            Ok(Outcome::Ignored)
        }
        // `LifecycleEvent` is #[non_exhaustive]: a variant here is one the
        // pinned client parses but this match was not taught (a client bump
        // without a handler). Not the same as `Unknown`: the event is
        // meaningful and probably needs cleanup, so acknowledging it would
        // drop it for good. Refuse it as retriable (503) so the platform keeps
        // redelivering until a release that handles it is deployed.
        other => {
            tracing::error!(event_id = %event.id, kind = ?other,
                "a platform event this build parses but has no handler for; refusing it so \
                 the platform redelivers it until a release handles it");
            Err(Error::Unavailable)
        }
    }
}

async fn first_delivery(tx: &mut Tx<'_>, event: &WebhookEvent, kind: &str) -> Result<bool> {
    let org = tx.org();
    let inserted = sqlx::query(
        "INSERT INTO platform_events (event_id, org_id, kind) VALUES ($1, $2, $3) \
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(event.id)
    .bind(org)
    .bind(kind)
    .execute(tx.conn())
    .await?
    .rows_affected();
    Ok(inserted == 1)
}

async fn org_deleted(db: &Db, event: &WebhookEvent, org: OrgId) -> Result<Outcome> {
    // Tombstone first, on its own, so authentication refuses this org from now
    // on -- before, not as part of, the purge.
    sqlx::query("INSERT INTO deleted_orgs (org_id) VALUES ($1) ON CONFLICT (org_id) DO NOTHING")
        .bind(org)
        .execute(db.pool())
        .await?;

    // Then the purge, in short transactions: each takes the lifecycle lock
    // exclusively (so the first waits out any request already in flight;
    // later ones are refused by the tombstone) and deletes at most
    // PURGE_BATCH rows per table, children first. A table that still has rows
    // ends the transaction before its parents are touched, so nothing is
    // deleted by cascade and every row is counted where it lived.
    //
    // The dedupe marker is written only in the last transaction, the one
    // that finds every table empty: a crash part-way leaves no marker, so the
    // platform's redelivery runs the purge again. A duplicate re-runs it too
    // (it is idempotent, and catches anything that raced an earlier run).
    // Every statement names the org in addition to the pinned policy.
    let mut deleted: Vec<u64> = vec![0; PURGED_TABLES.len()];
    let first = loop {
        let mut tx = db.begin(org).await?;
        exclude_requests(&mut tx, org).await?;
        let mut drained = true;
        for (i, table) in PURGED_TABLES.iter().enumerate() {
            let n = sqlx::query(&format!(
                "DELETE FROM {table} WHERE org_id = $1 AND ctid = ANY(ARRAY( \
                   SELECT ctid FROM {table} WHERE org_id = $1 LIMIT $2))"
            ))
            .bind(org)
            .bind(PURGE_BATCH)
            .execute(tx.conn())
            .await?
            .rows_affected();
            deleted[i] += n;
            if n as i64 >= PURGE_BATCH {
                drained = false;
                break;
            }
        }
        if drained {
            let first = first_delivery(&mut tx, event, "org.deleted").await?;
            tx.commit().await?;
            break first;
        }
        tx.commit().await?;
    };
    let deleted: serde_json::Map<String, serde_json::Value> = PURGED_TABLES
        .iter()
        .zip(deleted)
        .map(|(table, n)| ((*table).to_string(), n.into()))
        .collect();

    tracing::info!(org = %org, event_id = %event.id, first, ?deleted, "purged a deleted org's data");
    Ok(if first {
        Outcome::Applied {
            detail: deleted.into(),
        }
    } else {
        Outcome::Duplicate
    })
}

async fn member_removed(
    db: &Db,
    event: &WebhookEvent,
    org: OrgId,
    user: UserId,
) -> Result<Outcome> {
    let mut tx = db.begin(org).await?;
    exclude_requests(&mut tx, org).await?;
    let first = first_delivery(&mut tx, event, "member.removed").await?;
    let mut sessions = 0;
    if first {
        // Only on the first delivery: a late redelivery must not restart the
        // refusal window of a member who has since been re-invited, nor sign
        // them out of the console again.
        sqlx::query(
            "INSERT INTO removed_members (org_id, user_id) VALUES ($1, $2) \
             ON CONFLICT (org_id, user_id) DO UPDATE SET removed_at = now()",
        )
        .bind(org)
        .bind(user)
        .execute(tx.conn())
        .await?;
        sessions = crate::console_sessions::delete_for_user(&mut tx, user).await?;
    }
    tx.commit().await?;

    tracing::info!(org = %org, user = %user, first, sessions,
        "member removed; their tokens are refused and their console sessions deleted");
    Ok(if first {
        Outcome::Applied {
            detail: serde_json::json!({ "console_sessions": sessions }),
        }
    } else {
        Outcome::Duplicate
    })
}

async fn team_deleted(db: &Db, event: &WebhookEvent, org: OrgId, team: Uuid) -> Result<Outcome> {
    let mut tx = db.begin(org).await?;
    // Exclusive, so a catalog write that already holds the org's shared lock
    // (opened through `begin_live`) commits first and is nulled with the
    // rest. That is all the lock guarantees: a write that verified the team
    // at the platform just before it was deleted, but opens its transaction
    // after this one commits, can still store the stale team_id. Accepted:
    // the window is one request, and the stale id stays until the entity is
    // next edited.
    exclude_requests(&mut tx, org).await?;
    if !first_delivery(&mut tx, event, "team.deleted").await? {
        tx.commit().await?;
        return Ok(Outcome::Duplicate);
    }
    let mut nulled = serde_json::Map::new();
    for table in TEAM_OWNED_TABLES {
        let n = sqlx::query(&format!(
            "UPDATE {table} SET team_id = NULL WHERE org_id = $1 AND team_id = $2"
        ))
        .bind(org)
        .bind(team)
        .execute(tx.conn())
        .await?
        .rows_affected();
        nulled.insert((*table).to_string(), n.into());
    }
    tx.commit().await?;

    tracing::info!(org = %org, team = %team, ?nulled, "team deleted; its catalog entities are unowned");
    Ok(Outcome::Applied {
        detail: nulled.into(),
    })
}
