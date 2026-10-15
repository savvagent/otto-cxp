//! The web console's sign-in sessions: a platform token pair held for a
//! browser.
//!
//! The console signs people in with an OAuth authorization-code + PKCE flow
//! against the otto platform (`cxp_http::console`). What that leaves here is a
//! [`Session`]: the org and user the platform vouched for, and the access and
//! refresh tokens it issued, sealed with `CXP_ENCRYPTION_KEY`. The browser
//! holds only a random cookie; the database holds only that cookie's SHA-256
//! ([`hash_cookie`]), so neither a dump nor a log line can be replayed as a
//! sign-in.
//!
//! This module is SQL only: it knows nothing of HTTP or of the platform's
//! token endpoint. The refresh protocol lives in `cxp_http::console`, which
//! uses [`lock`] to make it single-flight.
//!
//! # Tenancy
//!
//! `console_sessions` is a tenant table. A request arrives with a cookie and
//! no org, and the cookie is how the org is learned, so [`org_of`] asks the
//! database function `console_session_org`, which answers with the org of the
//! one unexpired session whose hash it is given and never a token (see
//! migration 0004). Everything else runs pinned to that org through
//! [`crate::platform_events::begin_live`], and names the org in its predicate
//! as well; the expiry sweep runs under [`crate::maintenance`].

use chrono::{DateTime, Utc};
use otto_tenant::crypto::Cipher;
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::{Db, Tx};
use sha2::{Digest, Sha256};

use crate::error::Result;
use crate::platform_events::begin_live;

/// A session whose access token expires within this long is refreshed before
/// it is used, so the token is still good for the request that triggered it.
pub const REFRESH_SKEW_SECS: i64 = 60;

/// The most expired sessions one sweep transaction deletes.
pub const SWEEP_BATCH: i64 = 1000;

/// The AES-GCM nonce length: a sealed column is `nonce || ciphertext`.
const NONCE_BYTES: usize = 12;

const COLS: &str = "id_hash, org_id, user_id, access_token_enc, refresh_token_enc, \
                    access_expires_at, created_at, expires_at";

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
}

// Not derived: the sealed tokens stay out of log lines.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("org_id", &self.org_id)
            .field("user_id", &self.user_id)
            .field("access_expires_at", &self.access_expires_at)
            .field("expires_at", &self.expires_at)
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
    let sealed = cipher.seal(plain.as_bytes())?;
    let mut out = sealed.nonce;
    out.extend_from_slice(&sealed.ciphertext);
    Ok(out)
}

fn open(cipher: &Cipher, stored: &[u8]) -> Result<String> {
    if stored.len() <= NONCE_BYTES {
        return Err(
            otto_tenant::Error::Crypto("a sealed session token is truncated".into()).into(),
        );
    }
    let (nonce, ciphertext) = stored.split_at(NONCE_BYTES);
    let plain = cipher.open(ciphertext, nonce)?;
    String::from_utf8(plain).map_err(|_| {
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

const FIND: &str = "SELECT {COLS} FROM console_sessions \
                    WHERE org_id = $1 AND id_hash = $2 AND expires_at > now()";

/// The unexpired session `id_hash` in `org`, read pinned to it.
pub async fn load(db: &Db, org: OrgId, id_hash: &[u8; 32]) -> Result<Option<Session>> {
    let mut tx = begin_live(db, org, None).await?;
    let session = sqlx::query_as(&FIND.replace("{COLS}", COLS))
        .bind(org)
        .bind(&id_hash[..])
        .fetch_optional(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(session)
}

/// Lock one session's row for the length of the returned transaction: the
/// single-flight guard for a token refresh. A concurrent caller blocks here
/// until the holder commits, then reads the rotated tokens. `None` means the
/// session is gone (signed out, swept, or its member removed).
pub async fn lock(
    db: &Db,
    org: OrgId,
    id_hash: &[u8; 32],
) -> Result<Option<(Tx<'static>, Session)>> {
    let mut tx = begin_live(db, org, None).await?;
    let session: Option<Session> =
        sqlx::query_as(&format!("{} FOR UPDATE", FIND.replace("{COLS}", COLS)))
            .bind(org)
            .bind(&id_hash[..])
            .fetch_optional(tx.conn())
            .await?;
    Ok(session.map(|s| (tx, s)))
}

/// Store a rotated token pair on a session held by [`lock`].
pub async fn rotate(
    tx: &mut Tx<'_>,
    cipher: &Cipher,
    id_hash: &[u8; 32],
    access_token: &str,
    refresh_token: &str,
    access_expires_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
) -> Result<()> {
    let org = tx.org();
    sqlx::query(
        "UPDATE console_sessions \
         SET access_token_enc = $3, refresh_token_enc = $4, access_expires_at = $5, \
             expires_at = $6 \
         WHERE org_id = $1 AND id_hash = $2",
    )
    .bind(org)
    .bind(&id_hash[..])
    .bind(seal(cipher, access_token)?)
    .bind(seal(cipher, refresh_token)?)
    .bind(access_expires_at)
    .bind(expires_at)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Delete a session inside a transaction that holds it.
pub async fn delete_locked(tx: &mut Tx<'_>, id_hash: &[u8; 32]) -> Result<u64> {
    let org = tx.org();
    Ok(
        sqlx::query("DELETE FROM console_sessions WHERE org_id = $1 AND id_hash = $2")
            .bind(org)
            .bind(&id_hash[..])
            .execute(tx.conn())
            .await?
            .rows_affected(),
    )
}

/// Delete one session. Idempotent.
pub async fn delete(db: &Db, org: OrgId, id_hash: &[u8; 32]) -> Result<u64> {
    let mut tx = begin_live(db, org, None).await?;
    let n = delete_locked(&mut tx, id_hash).await?;
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
