//! The web console's sign-in sessions: a platform token pair held for a
//! browser.
//!
//! The console signs people in with an OAuth authorization-code + PKCE flow
//! against the otto platform (`cxp_http::console`). What that leaves here is a
//! [`Session`]: the org and user the platform vouched for, and the access and
//! refresh tokens it issued, sealed ([`crate::sealed`]) with the console's
//! session-token key. The browser holds only a random cookie; the database
//! holds only that cookie's SHA-256 ([`hash_cookie`]), so neither a dump nor a
//! log line can be replayed as a sign-in.
//!
//! This module is SQL only: it knows nothing of HTTP or of the platform's
//! token endpoint.
//!
//! # Refreshing without holding anything
//!
//! The platform rotates the refresh token on every use, so two refreshes of
//! one session must never race. A refresh is three steps, and no connection,
//! row lock, or lifecycle lock is held across the platform call between them:
//!
//! 1. [`claim_refresh`], one short transaction: take the session's refresh
//!    lease (`refresh_lease_until`) and read its sealed refresh token, or learn
//!    that the session is gone, already fresh, or leased by someone else.
//! 2. The caller asks the platform, holding nothing.
//! 3. [`store_refreshed`], a second short transaction: store the new pair only
//!    if the session's `generation` is still the one claimed (a
//!    compare-and-swap), which also ends the lease. A sign-out or a purge in
//!    between wins: the row is gone, and nothing is resurrected.
//!
//! Within one process the caller also coalesces refreshes in memory, so
//! parallel requests wait without a connection; the lease is what keeps two
//! replicas apart.
//!
//! # Tenancy
//!
//! `console_sessions` is a tenant table. A request arrives with a cookie and
//! no org, and the cookie is how the org is learned, so [`org_of`] asks the
//! database function `console_session_org`, which answers with the org of the
//! one unexpired session whose hash it is given and never a token (see
//! migration 0003). Everything else runs pinned to that org through
//! [`crate::platform_events::begin_live`], and names the org in its predicate
//! as well; the expiry sweep runs under [`crate::maintenance`].

use chrono::{DateTime, Utc};
use otto_tenant::crypto::Cipher;
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::{Db, Tx};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::platform_events::begin_live;
use crate::sealed;

/// A session whose access token expires within this long is refreshed before
/// it is used, so the token is still good for the request that triggered it.
pub const REFRESH_SKEW_SECS: i64 = 60;

/// The most expired sessions one sweep transaction deletes.
pub const SWEEP_BATCH: i64 = 1000;

macro_rules! cols {
    () => {
        "id_hash, org_id, user_id, access_token_enc, refresh_token_enc, \
         access_expires_at, created_at, expires_at, refresh_lease_until, generation"
    };
}

/// The unexpired session `$2` in org `$1`.
const FIND: &str = concat!(
    "SELECT ",
    cols!(),
    " FROM console_sessions WHERE org_id = $1 AND id_hash = $2 AND expires_at > now()"
);

/// The database key for a cookie value.
pub fn hash_cookie(cookie: &str) -> [u8; 32] {
    Sha256::digest(cookie.as_bytes()).into()
}

/// A stored session. The tokens are still sealed; open them with
/// [`Session::access_token`] and [`Session::refresh_token`].
#[derive(Clone, sqlx::FromRow)]
pub struct Session {
    pub id_hash: Vec<u8>,
    pub org_id: OrgId,
    pub user_id: UserId,
    access_token_enc: Vec<u8>,
    refresh_token_enc: Vec<u8>,
    pub access_expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Until when a refresh holds this session's lease, if one does.
    pub refresh_lease_until: Option<DateTime<Utc>>,
    /// Bumped on every rotation.
    pub generation: i64,
}

// Not derived: the sealed tokens stay out of log lines.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("org_id", &self.org_id)
            .field("user_id", &self.user_id)
            .field("access_expires_at", &self.access_expires_at)
            .field("expires_at", &self.expires_at)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn access_token(&self, cipher: &Cipher) -> Result<String> {
        open(cipher, &self.access_token_enc)
    }

    pub fn refresh_token(&self, cipher: &Cipher) -> Result<String> {
        open(cipher, &self.refresh_token_enc)
    }

    /// Whether the access token is expired or about to be.
    pub fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        self.access_expires_at - chrono::Duration::seconds(REFRESH_SKEW_SECS) <= now
    }
}

fn seal(cipher: &Cipher, plain: &str) -> Result<Vec<u8>> {
    sealed::seal(cipher, plain.as_bytes())
}

fn open(cipher: &Cipher, stored: &[u8]) -> Result<String> {
    String::from_utf8(sealed::open(cipher, stored)?).map_err(|_| {
        otto_tenant::Error::Crypto("a sealed session token is not UTF-8".into()).into()
    })
}

/// A new session's contents. The tokens are plaintext here and sealed on
/// insert.
pub struct NewSession<'a> {
    pub id_hash: [u8; 32],
    pub org_id: OrgId,
    pub user_id: UserId,
    pub access_token: &'a str,
    pub refresh_token: &'a str,
    pub access_expires_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Store a session. Refused with `AccessRevoked` if the org was deleted or
/// the user removed: a sign-in that completes while the platform's cleanup
/// runs must not leave a session behind it.
pub async fn create(db: &Db, cipher: &Cipher, new: &NewSession<'_>) -> Result<()> {
    let access = seal(cipher, new.access_token)?;
    let refresh = seal(cipher, new.refresh_token)?;
    let mut tx = begin_live(db, new.org_id, Some(new.user_id)).await?;
    sqlx::query(
        "INSERT INTO console_sessions \
           (id_hash, org_id, user_id, access_token_enc, refresh_token_enc, \
            access_expires_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&new.id_hash[..])
    .bind(new.org_id)
    .bind(new.user_id)
    .bind(access)
    .bind(refresh)
    .bind(new.access_expires_at)
    .bind(new.expires_at)
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The org of the unexpired session whose cookie hashes to `id_hash`. The one
/// lookup that runs before an org is known; it learns nothing else.
pub async fn org_of(db: &Db, id_hash: &[u8; 32]) -> Result<Option<OrgId>> {
    let org: Option<uuid::Uuid> = sqlx::query_scalar("SELECT console_session_org($1)")
        .bind(&id_hash[..])
        .fetch_one(db.pool())
        .await?;
    Ok(org.map(OrgId::from))
}

async fn find(tx: &mut Tx<'_>, id_hash: &[u8; 32]) -> Result<Option<Session>> {
    let org = tx.org();
    Ok(sqlx::query_as(FIND)
        .bind(org)
        .bind(&id_hash[..])
        .fetch_optional(tx.conn())
        .await?)
}

/// The unexpired session `id_hash` in `org`, read pinned to it.
pub async fn load(db: &Db, org: OrgId, id_hash: &[u8; 32]) -> Result<Option<Session>> {
    let mut tx = begin_live(db, org, None).await?;
    let session = find(&mut tx, id_hash).await?;
    tx.commit().await?;
    Ok(session)
}

/// What [`claim_refresh`] found.
#[derive(Debug)]
pub enum Claim {
    /// This caller holds the lease until it stores a new pair, gives the lease
    /// back, or the lease lapses. Refresh with the session's refresh token,
    /// then [`store_refreshed`] against its `generation`.
    Claimed(Session),
    /// The access token is good: someone else refreshed it.
    Fresh(Session),
    /// Another refresh holds a live lease: wait, then look again.
    Leased,
    /// Signed out, swept, purged, or expired.
    Gone,
}

/// Take the refresh lease on session `id_hash` for `lease`, if its token
/// still needs a refresh at `now` and no live lease is held. One short
/// transaction; nothing is held when it returns.
pub async fn claim_refresh(
    db: &Db,
    org: OrgId,
    id_hash: &[u8; 32],
    now: DateTime<Utc>,
    lease: chrono::Duration,
) -> Result<Claim> {
    let mut tx = begin_live(db, org, None).await?;
    let claimed: Option<Session> = sqlx::query_as(concat!(
        "UPDATE console_sessions SET refresh_lease_until = $3 + $4 \
         WHERE org_id = $1 AND id_hash = $2 AND expires_at > now() \
           AND access_expires_at <= $3 + $5 \
           AND (refresh_lease_until IS NULL OR refresh_lease_until <= $3) \
         RETURNING ",
        cols!()
    ))
    .bind(org)
    .bind(&id_hash[..])
    .bind(now)
    .bind(lease)
    .bind(chrono::Duration::seconds(REFRESH_SKEW_SECS))
    .fetch_optional(tx.conn())
    .await?;
    let claim = match claimed {
        Some(s) => Claim::Claimed(s),
        None => match find(&mut tx, id_hash).await? {
            None => Claim::Gone,
            Some(s) if !s.needs_refresh(now) => Claim::Fresh(s),
            Some(_) => Claim::Leased,
        },
    };
    tx.commit().await?;
    Ok(claim)
}

/// A refreshed token pair for a claimed session.
pub struct Refreshed<'a> {
    pub access_token: &'a str,
    pub refresh_token: &'a str,
    pub access_expires_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Store a refreshed pair on session `id_hash`, if it is still at
/// `generation` (the claim's): bumps the generation and ends the lease.
/// `false` means the session was deleted or rotated by someone else since the
/// claim, and nothing was written.
pub async fn store_refreshed(
    db: &Db,
    cipher: &Cipher,
    org: OrgId,
    id_hash: &[u8; 32],
    generation: i64,
    new: &Refreshed<'_>,
) -> Result<bool> {
    let access = seal(cipher, new.access_token)?;
    let refresh = seal(cipher, new.refresh_token)?;
    let mut tx = begin_live(db, org, None).await?;
    let n = sqlx::query(
        "UPDATE console_sessions \
         SET access_token_enc = $4, refresh_token_enc = $5, access_expires_at = $6, \
             expires_at = $7, generation = generation + 1, refresh_lease_until = NULL \
         WHERE org_id = $1 AND id_hash = $2 AND generation = $3",
    )
    .bind(org)
    .bind(&id_hash[..])
    .bind(generation)
    .bind(access)
    .bind(refresh)
    .bind(new.access_expires_at)
    .bind(new.expires_at)
    .execute(tx.conn())
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(n == 1)
}

/// Give back a lease taken at `generation` without rotating (the platform
/// could not be reached), so the next request may try again at once.
pub async fn release_lease(db: &Db, org: OrgId, id_hash: &[u8; 32], generation: i64) -> Result<()> {
    let mut tx = begin_live(db, org, None).await?;
    sqlx::query(
        "UPDATE console_sessions SET refresh_lease_until = NULL \
         WHERE org_id = $1 AND id_hash = $2 AND generation = $3",
    )
    .bind(org)
    .bind(&id_hash[..])
    .bind(generation)
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Delete session `id_hash` if it is still at `generation`: the platform
/// refused the refresh token that generation holds.
pub async fn delete_generation(
    db: &Db,
    org: OrgId,
    id_hash: &[u8; 32],
    generation: i64,
) -> Result<u64> {
    let mut tx = begin_live(db, org, None).await?;
    let n = sqlx::query(
        "DELETE FROM console_sessions WHERE org_id = $1 AND id_hash = $2 AND generation = $3",
    )
    .bind(org)
    .bind(&id_hash[..])
    .bind(generation)
    .execute(tx.conn())
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(n)
}

/// Delete one session and return what it held (sign-out: the caller revokes
/// its refresh token afterwards, with nothing held). Idempotent: `None` if
/// there was no such session.
pub async fn take(db: &Db, org: OrgId, id_hash: &[u8; 32]) -> Result<Option<Session>> {
    let mut tx = begin_live(db, org, None).await?;
    let session = sqlx::query_as(concat!(
        "DELETE FROM console_sessions WHERE org_id = $1 AND id_hash = $2 RETURNING ",
        cols!()
    ))
    .bind(org)
    .bind(&id_hash[..])
    .fetch_optional(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(session)
}

/// Delete one session. Idempotent.
pub async fn delete(db: &Db, org: OrgId, id_hash: &[u8; 32]) -> Result<u64> {
    let mut tx = begin_live(db, org, None).await?;
    let n = sqlx::query("DELETE FROM console_sessions WHERE org_id = $1 AND id_hash = $2")
        .bind(org)
        .bind(&id_hash[..])
        .execute(tx.conn())
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(n)
}

/// Delete every session `user` holds in the transaction's org (the
/// `member.removed` cleanup, in its own transaction).
pub(crate) async fn delete_for_user(tx: &mut Tx<'_>, user: UserId) -> Result<u64> {
    let org = tx.org();
    Ok(
        sqlx::query("DELETE FROM console_sessions WHERE org_id = $1 AND user_id = $2")
            .bind(org)
            .bind(user)
            .execute(tx.conn())
            .await?
            .rows_affected(),
    )
}

/// Delete expired sessions across every org, in maintenance transactions of
/// at most [`SWEEP_BATCH`] rows each, until none are left. Background code
/// only (the hourly sweep, [`crate::platform_events::sweep`]).
pub async fn sweep_expired(db: &Db) -> Result<u64> {
    let mut swept = 0;
    loop {
        let mut tx = crate::maintenance::begin(db).await?;
        let n = sqlx::query(
            "DELETE FROM console_sessions WHERE id_hash IN ( \
               SELECT id_hash FROM console_sessions WHERE expires_at <= now() LIMIT $1)",
        )
        .bind(SWEEP_BATCH)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        swept += n;
        if (n as i64) < SWEEP_BATCH {
            return Ok(swept);
        }
    }
}
