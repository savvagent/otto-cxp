//! The web console's sign-in: OAuth authorization code + PKCE against the
//! otto platform, and the session cookie that `/api` accepts afterwards.
//!
//! ```text
//!  browser                     otto-cxp                                 otto platform
//!    │ GET /auth/login?next=/console/x  state + PKCE verifier -> sealed cookie
//!    │◄─ 303 + __Host-cxp_oauth ──┤                                          │
//!    │ GET /oauth/authorize?client_id&code_challenge&resource&scope&state ──►│ sign in, pick org
//!    │◄──────────────────────────────────── 303 /auth/callback?code&state ───┤
//!    │ GET /auth/callback ───────►│ POST /oauth/token (code, verifier) ─────►│
//!    │                            │◄── access + refresh ─────────────────────┤
//!    │                            │ introspect + tombstones; store session   │
//!    │◄─ 303 next (under /console/) + __Host-cxp_session                     │
//! ```
//!
//! The browser only ever holds two opaque cookies ([`cookies`]); the
//! platform's tokens stay in `console_sessions`, sealed
//! (`cxp_core::console_sessions`). The session cookie is a credential only
//! where a surface opts in ([`crate::Authenticator::with_console`]): `/api`,
//! never `/mcp`.
//!
//! # Keys
//!
//! `CXP_ENCRYPTION_KEY` is never used directly. [`ConsoleKeys::derive`] takes
//! two independent subkeys from it with HKDF-SHA256: one seals the stored
//! token pairs (`cxp.console.session-tokens.v1`), the other the oauth cookie
//! (`cxp.console.oauth-flow.v1`), so a value sealed for one use never opens as
//! the other. Rotating the key signs everyone out: stored tokens no longer
//! open, and such a session is dropped as dead.
//!
//! # Refresh: coalesced in memory, leased across replicas, nothing held
//!
//! The platform **rotates the refresh token on every use, and presenting a
//! spent one revokes the whole family**, so two refreshes of one session
//! would sign the person out. A refresh must also not hold a database
//! connection or lock while it waits on the platform: the pool is five
//! connections for every org, and an `org.deleted` purge takes the org's
//! lifecycle lock exclusively.
//!
//! - **In this process**, refreshes of one session are coalesced: the first
//!   request starts one, and every other request for that session awaits the
//!   same result, holding no connection.
//! - **Across replicas**, the refresh claims a lease on the row
//!   (`cxp_core::console_sessions::claim_refresh`, one short transaction),
//!   calls the platform with nothing held, and stores the new pair as a
//!   compare-and-swap on the row's generation (a second short transaction).
//!   A replica that finds a live lease polls the row a bounded number of
//!   times ([`LEASE_POLL`], [`LEASE_POLLS`]) for the fresh token.
//! - The refresh runs in a spawned task: once the platform has rotated the
//!   pair, storing it is not optional, and a handler future is dropped when
//!   its client disconnects. The task frees its single-flight entry from a
//!   drop guard, so even a panic in it leaves the session refreshable by the
//!   next request rather than answering `503` until a restart.
//!
//! ## Why one refresh token is never presented twice
//!
//! The lease lives on the database's clock (`clock_timestamp()` when the
//! claiming `UPDATE` runs; see `cxp_core::console_sessions`), and the
//! holder's store and release match the exact lease it claimed (generation
//! and expiry), so a holder whose lease lapsed and was taken over can neither
//! store over nor free the newer claim. What remains is to make the holder
//! finish before its lease can lapse, which the timeouts bound:
//!
//! - The lease is [`LEASE`] = 45 s, starting no earlier than the moment the
//!   holder began its claim (it is set by the `UPDATE`, after any wait for a
//!   connection). The holder measures from that moment with a monotonic
//!   clock, so the two clocks are compared only as durations.
//! - Right before calling the platform, the holder requires
//!   `elapsed + PLATFORM_TIMEOUT (10 s) + STORE_ALLOWANCE (20 s) <= LEASE`;
//!   otherwise it gives the lease back unspent and answers `503`. A claim may
//!   therefore take up to 15 s and still go ahead.
//! - The platform call is cut off at `PLATFORM_TIMEOUT` (the whole request,
//!   not just the connect).
//! - The store then has at least `STORE_ALLOWANCE` = 20 s for its
//!   compare-and-swap `UPDATE` to take the row, and the database enforces
//!   what it may spend: the pool's acquire timeout
//!   (`cxp_core::POOL_ACQUIRE_TIMEOUT`, 5 s), then a transaction that sets
//!   `cxp_core::console_sessions::STORE_TIMEOUTS` (a 3 s `statement_timeout`,
//!   a 2 s `lock_timeout`) before it waits on anything, and runs
//!   `STORE_STATEMENTS` = 4 statements under them (the shared lifecycle lock,
//!   the revocation check, the `UPDATE`, the `COMMIT`): at most 5 + 4 x 3 =
//!   17 s, checked at compile time. The transaction's opening statements,
//!   before the bounds are set, wait on nothing; that is assumed, not
//!   enforced. Once the `UPDATE` holds the row, a competing claim waits on its
//!   row lock and then sees the rotated, fresh session. The org's lifecycle
//!   lock is taken exclusively by an `org.deleted`, `member.removed`, or
//!   `team.deleted` cleanup; the store times out behind one rather than
//!   waiting it out, and fails as a store against a failed database does
//!   (below).
//!
//! So no second replica can claim, and present the same refresh token, while
//! the first could still store its result. If the store fails instead (the
//! database is down), the new pair is revoked and lost and the stored refresh
//! token is spent: the next claim presents it, the platform refuses it
//! (`invalid_grant`), the session is deleted, and the person signs in again.
//! A waiting replica polls for a little longer than one lease
//! ([`LEASE_POLL`] x [`LEASE_POLLS`] = 47.5 s), so a lease whose holder died
//! lapses and is taken over before it gives up.
//!
//! | Platform says | Session | Request |
//! |---|---|---|
//! | a new pair | rotated in place | proceeds |
//! | `invalid_grant` | deleted | `401`: sign in again |
//! | unreachable, 5xx | untouched, lease given back | `503`, never `401` |
//! | any other refusal | untouched, lease given back | `500`: the client registration or configuration is wrong |

mod cookies;
mod oauth;
mod routes;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{DateTime, Utc};
use cxp_core::auth::{self, AuthFailure, Principal};
use cxp_core::console_sessions::{self, Claim, Lease, Refreshed, Session, REFRESH_SKEW_SECS};
use futures::future::{BoxFuture, FutureExt, Shared};
use hkdf::Hkdf;
use http::{HeaderMap, Method};
use otto_resource::PlatformClient;
use otto_tenant::crypto::Cipher;
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use sha2::Sha256;

pub use cookies::same_origin;
pub use oauth::pkce_challenge;
pub(crate) use routes::cross_site;
pub use routes::{router, safe_next, OAUTH_MAX_AGE_SECS};

use cookies::{is_token_shaped, origin_of, read_cookie, Cookies};
use oauth::{PlatformOAuth, TokenError};

/// What the console asks the platform for: every scope its routes check. The
/// platform grants the subset the person may have, and the routes check the
/// grant as they do for a bearer.
pub const CONSOLE_SCOPES: &[&str] = &[cxp_core::scopes::CXP_READ, cxp_core::scopes::CXP_WRITE];

/// Where the console lives, and where a sign-in lands by default.
pub const CONSOLE_HOME: &str = "/console/";

/// How long a session lasts without a refresh: the platform's refresh-token
/// lifetime. Each rotation issues a fresh refresh token and restarts it.
pub const SESSION_TTL_DAYS: i64 = 30;

/// Used when the platform's token response omits `expires_in`.
const DEFAULT_ACCESS_TTL_SECS: i64 = 3600;

/// How long a refresh lease lasts, on the database's clock: long enough for a
/// claim, one platform call ([`oauth::PLATFORM_TIMEOUT`]), and the store
/// ([`STORE_ALLOWANCE`]), so a live refresh never loses it (see the module
/// docs), and short enough that a replica that died mid-refresh blocks no one
/// for long.
pub const LEASE: Duration = Duration::from_secs(45);

/// What a lease holder keeps back for storing the new pair after the platform
/// answers. The store cannot spend more: the pool's acquire timeout
/// ([`cxp_core::POOL_ACQUIRE_TIMEOUT`]) plus
/// [`console_sessions::STORE_STATEMENTS`] statements, each cut off by
/// [`console_sessions::STORE_TIMEOUTS`] in the database, fit inside it
/// (checked at compile time below).
pub const STORE_ALLOWANCE: Duration = Duration::from_secs(20);

/// How often a request that found another replica's lease looks again.
pub const LEASE_POLL: Duration = Duration::from_millis(250);

/// How many times it looks before answering `503`: a little past one lease,
/// so a lease whose holder died lapses and is taken over first.
pub const LEASE_POLLS: u32 = 190;

// The arithmetic in the module docs, checked at compile time.
const _: () = {
    let budget = oauth::PLATFORM_TIMEOUT.as_millis() + STORE_ALLOWANCE.as_millis();
    assert!(
        budget < LEASE.as_millis(),
        "a claim must leave room for the call and the store"
    );
    // u128 milliseconds: `Duration` arithmetic is not const.
    let store = cxp_core::POOL_ACQUIRE_TIMEOUT.as_millis()
        + console_sessions::STORE_TIMEOUTS.statement.as_millis()
            * console_sessions::STORE_STATEMENTS as u128;
    assert!(
        store <= STORE_ALLOWANCE.as_millis(),
        "the store's database bounds must fit in what the holder keeps back"
    );
    assert!(
        LEASE_POLL.as_millis() * LEASE_POLLS as u128 > LEASE.as_millis(),
        "a waiting replica must outwait a dead holder's lease"
    );
};

/// Whether a holder whose claim began `elapsed` ago may still call the
/// platform: the call and the store must both fit in what is left of the
/// lease.
fn fits_in_lease(elapsed: Duration) -> bool {
    elapsed + oauth::PLATFORM_TIMEOUT + STORE_ALLOWANCE <= LEASE
}

/// HKDF-SHA256 `info` labels for the two subkeys of `CXP_ENCRYPTION_KEY`.
pub const SESSION_TOKENS_LABEL: &str = "cxp.console.session-tokens.v1";
pub const OAUTH_FLOW_LABEL: &str = "cxp.console.oauth-flow.v1";

/// The two keys the console seals with, each derived from
/// `CXP_ENCRYPTION_KEY` for one use only (see the module docs).
#[derive(Clone)]
pub struct ConsoleKeys {
    /// Seals the platform token pairs stored in `console_sessions`.
    pub session_tokens: Cipher,
    /// Seals the in-flight sign-in in the oauth cookie.
    pub oauth_flow: Cipher,
}

impl ConsoleKeys {
    /// Derive both keys from `CXP_ENCRYPTION_KEY`: base64 (standard or
    /// URL-safe) of exactly 32 bytes. The error names the variable, never its
    /// value.
    pub fn derive(encoded: &str) -> Result<Self, &'static str> {
        const BAD: &str = "CXP_ENCRYPTION_KEY must be base64 of exactly 32 bytes";
        let raw = STANDARD
            .decode(encoded.trim())
            .or_else(|_| URL_SAFE_NO_PAD.decode(encoded.trim()))
            .map_err(|_| BAD)?;
        if raw.len() != 32 {
            return Err(BAD);
        }
        let hk = Hkdf::<Sha256>::new(None, &raw);
        let subkey = |label: &str| {
            let mut okm = [0u8; 32];
            hk.expand(label.as_bytes(), &mut okm)
                .map_err(|_| "HKDF cannot expand 32 bytes")?;
            Cipher::from_base64_key(&STANDARD.encode(okm)).map_err(|_| BAD)
        };
        Ok(Self {
            session_tokens: subkey(SESSION_TOKENS_LABEL)?,
            oauth_flow: subkey(OAUTH_FLOW_LABEL)?,
        })
    }
}

/// What the console's sign-in needs to know about this deployment.
pub struct ConsoleSettings {
    /// `CXP_CONSOLE_CLIENT_ID`: the first-party public client registered at
    /// the platform with the redirect URI `{public_url}/auth/callback`.
    pub client_id: String,
    /// From `CXP_ENCRYPTION_KEY` ([`ConsoleKeys::derive`]).
    pub keys: ConsoleKeys,
    /// `CXP_PUBLIC_URL`, no trailing slash: the console's origin.
    pub public_url: String,
    /// `CXP_PLATFORM_URL`: where `/oauth/authorize` and `/oauth/token` are.
    pub platform_url: String,
    /// `CXP_RESOURCE_URI`: the `resource` every token is requested for.
    pub resource_uri: String,
}

/// A usable access token, and whose session it came from.
#[derive(Clone)]
struct Access {
    token: String,
    org: OrgId,
    user: UserId,
}

type RefreshResult = Result<Access, AuthFailure>;

/// The console's sign-in. Cheap to clone.
#[derive(Clone)]
pub struct Console {
    inner: Arc<Inner>,
}

struct Inner {
    db: Db,
    platform: Arc<PlatformClient>,
    oauth: PlatformOAuth,
    settings: ConsoleSettings,
    keys: ConsoleKeys,
    cookies: Cookies,
    /// `scheme://host[:port]` of the public URL, as a browser sends `Origin`.
    origin: String,
    /// The refresh in flight for each session, by cookie hash: the in-process
    /// single flight. An entry is removed when its refresh task ends, however
    /// it ends ([`InflightEntry`]).
    inflight: Inflight<Shared<BoxFuture<'static, RefreshResult>>>,
}

/// The in-process single flight: one entry per session being refreshed.
type Inflight<V> = Arc<Mutex<HashMap<[u8; 32], V>>>;

/// Removes its session's single-flight entry when dropped: when the refresh
/// task returns, and also when it panics, so a session is never left behind
/// an entry whose task is gone.
struct InflightEntry<V> {
    map: Inflight<V>,
    key: [u8; 32],
}

impl<V> Drop for InflightEntry<V> {
    fn drop(&mut self) {
        self.map
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.key);
    }
}

impl Console {
    pub fn new(db: Db, platform: Arc<PlatformClient>, settings: ConsoleSettings) -> Self {
        let public_url = settings.public_url.trim_end_matches('/').to_string();
        let secure = !public_url.starts_with("http://");
        let settings = ConsoleSettings {
            public_url,
            ..settings
        };
        Self {
            inner: Arc::new(Inner {
                oauth: PlatformOAuth::new(&settings.platform_url),
                origin: origin_of(&settings.public_url).unwrap_or_default(),
                cookies: Cookies { secure },
                keys: settings.keys.clone(),
                db,
                platform,
                settings,
                inflight: Inflight::default(),
            }),
        }
    }

    fn redirect_uri(&self) -> String {
        format!("{}/auth/callback", self.inner.settings.public_url)
    }

    /// The session cookie on this request, if it is shaped like one we mint.
    /// Anything else is never looked up.
    pub fn session_cookie<'a>(&self, headers: &'a HeaderMap) -> Option<&'a str> {
        read_cookie(headers, self.inner.cookies.session()).filter(|v| is_token_shaped(v))
    }

    /// Whether a non-safe request carrying the session cookie came from this
    /// origin (see [`same_origin`]).
    pub fn same_origin(&self, method: &Method, headers: &HeaderMap) -> bool {
        same_origin(&self.inner.origin, method, headers)
    }

    /// The caller behind the session cookie `cookie`: a platform access token
    /// good for this request (refreshed first if it is about to expire),
    /// checked exactly as a bearer token is (the platform's introspection for
    /// this service's audience, then the tombstones), and bound to the
    /// session: the token must open the session's own org, as its own user. A
    /// session whose token the platform no longer honours is deleted.
    pub async fn authenticate(&self, cookie: &str) -> Result<Principal, AuthFailure> {
        let id_hash = console_sessions::hash_cookie(cookie);
        let access = self.access_token(&id_hash).await?;
        let i = &self.inner;
        match auth::authenticate_token(&i.db, &i.platform, &access.token).await {
            Ok(p) if p.org_id == access.org && p.user_id == access.user => Ok(p),
            Ok(p) => {
                // The platform says this token belongs to another org or user
                // than the session was stored for. Nothing legitimate does
                // that.
                tracing::error!(session_org = %access.org, token_org = %p.org_id,
                    session_user = %access.user, token_user = %p.user_id,
                    "a console session's token is not the session's; dropping the session");
                self.forget(access.org, &id_hash).await;
                Err(AuthFailure::Inactive)
            }
            Err(AuthFailure::Inactive) => {
                // Revoked at the platform, or the member left: nothing will
                // revive it.
                self.forget(access.org, &id_hash).await;
                Err(AuthFailure::Inactive)
            }
            Err(e) => Err(e),
        }
    }

    /// A platform access token for the session `id_hash`.
    async fn access_token(&self, id_hash: &[u8; 32]) -> RefreshResult {
        let db = &self.inner.db;
        let org = console_sessions::org_of(db, id_hash)
            .await
            .map_err(|e| db_failure("look up a console session", e))?
            .ok_or(AuthFailure::Inactive)?;
        let session = console_sessions::load(db, org, id_hash)
            .await
            .map_err(|e| db_failure("read a console session", e))?
            .ok_or(AuthFailure::Inactive)?;
        if !session.needs_refresh(Utc::now()) {
            return self.open_access(&session, id_hash).await;
        }
        self.refresh_once(org, *id_hash).await
    }

    /// Join the refresh in flight for `id_hash` in this process, or start one.
    /// Waiting holds no connection.
    async fn refresh_once(&self, org: OrgId, id_hash: [u8; 32]) -> RefreshResult {
        let shared = {
            let mut inflight = self
                .inner
                .inflight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inflight
                .entry(id_hash)
                .or_insert_with(|| {
                    // Detached: see the module docs. It removes its own entry
                    // when it ends, which waits for this lock to be released.
                    let this = self.clone();
                    let entry = InflightEntry {
                        map: Arc::clone(&self.inner.inflight),
                        key: id_hash,
                    };
                    let task = tokio::spawn(async move {
                        let _entry = entry;
                        this.refresh(org, id_hash).await
                    });
                    async move {
                        task.await.unwrap_or_else(|e| {
                            tracing::error!(error = %e, "the console session refresh task failed");
                            Err(AuthFailure::Unavailable)
                        })
                    }
                    .boxed()
                    .shared()
                })
                .clone()
        };
        shared.await
    }

    /// The refresh protocol across replicas (see the module docs).
    async fn refresh(&self, org: OrgId, id_hash: [u8; 32]) -> RefreshResult {
        let db = &self.inner.db;
        let lease = chrono::Duration::from_std(LEASE).unwrap_or(chrono::Duration::seconds(45));
        for _ in 0..=LEASE_POLLS {
            // The lease starts no earlier than this (module docs).
            let started = Instant::now();
            let claim = console_sessions::claim_refresh(db, org, &id_hash, Utc::now(), lease)
                .await
                .map_err(|e| db_failure("claim a console session's refresh", e))?;
            match claim {
                Claim::Claimed(session, lease) => {
                    return self.refresh_claimed(session, lease, started, id_hash).await
                }
                Claim::Fresh(session) => return self.open_access(&session, &id_hash).await,
                // Signed out, swept, or its member removed since it was found.
                Claim::Gone => return Err(AuthFailure::Inactive),
                Claim::Leased => tokio::time::sleep(LEASE_POLL).await,
            }
        }
        tracing::warn!(org = %org,
            "another replica's refresh of a console session did not finish in time; answering 503");
        Err(AuthFailure::Unavailable)
    }

    /// Refresh a session this caller holds the lease on. Nothing is held while
    /// the platform answers.
    async fn refresh_claimed(
        &self,
        session: Session,
        lease: Lease,
        started: Instant,
        id_hash: [u8; 32],
    ) -> RefreshResult {
        let i = &self.inner;
        let (org, user) = (session.org_id, session.user_id);
        let Ok(refresh_token) = session.refresh_token(&i.keys.session_tokens) else {
            tracing::warn!(
                "a console session's tokens cannot be opened (was CXP_ENCRYPTION_KEY \
                rotated?); dropping it"
            );
            self.forget(org, &id_hash).await;
            return Err(AuthFailure::Inactive);
        };
        if !fits_in_lease(started.elapsed()) {
            // The claim was slow: what is left of the lease might not cover
            // the call and the store. Give it back with the token unspent.
            tracing::warn!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                "claiming a console session's refresh took too long; answering 503"
            );
            self.release(org, &id_hash, lease).await;
            return Err(AuthFailure::Unavailable);
        }

        let answer = i
            .oauth
            .refresh(
                &i.settings.client_id,
                &refresh_token,
                &i.settings.resource_uri,
            )
            .await;
        match answer {
            Ok(tokens) => {
                let stored = console_sessions::store_refreshed(
                    &i.db,
                    &i.keys.session_tokens,
                    org,
                    &id_hash,
                    lease,
                    &Refreshed {
                        access_token: &tokens.access_token,
                        refresh_token: &tokens.refresh_token,
                        access_expires_at: access_expiry(tokens.expires_in),
                        expires_at: Utc::now() + chrono::Duration::days(SESSION_TTL_DAYS),
                    },
                )
                .await;
                match stored {
                    Ok(true) => Ok(Access {
                        token: tokens.access_token,
                        org,
                        user,
                    }),
                    // Signed out, purged, or claimed anew while the platform
                    // answered: the new pair has nowhere to go, so it must not
                    // outlive this.
                    Ok(false) | Err(cxp_core::Error::AccessRevoked) => {
                        self.revoke(&tokens.refresh_token).await;
                        Err(AuthFailure::Inactive)
                    }
                    Err(e) => {
                        // The platform has rotated the pair and it could not be
                        // stored: the session's refresh token is spent (the next
                        // claim is refused and signs the person out), and the new
                        // pair is unreachable, so it must not outlive this either.
                        let failure = db_failure("store a refreshed console session", e);
                        self.revoke(&tokens.refresh_token).await;
                        Err(failure)
                    }
                }
            }
            Err(TokenError::InvalidGrant) => {
                tracing::info!(org = %org,
                    "the platform refused a console session's refresh token; dropping the session");
                console_sessions::delete_generation(&i.db, org, &id_hash, lease.generation)
                    .await
                    .map_err(|e| db_failure("drop a console session", e))?;
                Err(AuthFailure::Inactive)
            }
            Err(TokenError::Unavailable(reason)) => {
                // Untouched: as far as we know the refresh token is unspent.
                tracing::warn!(%reason, "could not refresh a console session; answering 503");
                self.release(org, &id_hash, lease).await;
                Err(AuthFailure::Unavailable)
            }
            Err(TokenError::Rejected(reason)) => {
                tracing::error!(%reason,
                    "the platform refused the console's refresh request; is CXP_CONSOLE_CLIENT_ID \
                     registered at the platform?");
                self.release(org, &id_hash, lease).await;
                Err(AuthFailure::Internal)
            }
        }
    }

    /// Open a session's access token. One that cannot be opened (the key was
    /// rotated) is a dead session, not a server error.
    async fn open_access(&self, session: &Session, id_hash: &[u8; 32]) -> RefreshResult {
        match session.access_token(&self.inner.keys.session_tokens) {
            Ok(token) => Ok(Access {
                token,
                org: session.org_id,
                user: session.user_id,
            }),
            Err(_) => {
                tracing::warn!(
                    "a console session's token cannot be opened (was \
                    CXP_ENCRYPTION_KEY rotated?); dropping it"
                );
                self.forget(session.org_id, id_hash).await;
                Err(AuthFailure::Inactive)
            }
        }
    }

    /// End the session behind `cookie`: delete it and commit, then revoke its
    /// refresh token at the platform with nothing held (best effort: signing
    /// out must work while the platform is down). `Err` only when the
    /// database could not be asked; a session that is already gone is `Ok`.
    async fn end_session(&self, cookie: &str) -> Result<(), ()> {
        let i = &self.inner;
        let id_hash = console_sessions::hash_cookie(cookie);
        let org = match console_sessions::org_of(&i.db, &id_hash).await {
            Ok(Some(org)) => org,
            Ok(None) => return Ok(()),
            Err(e) => {
                tracing::error!(error = %e.log_detail(), "could not look up a console session to end");
                return Err(());
            }
        };
        let session = match console_sessions::take(&i.db, org, &id_hash).await {
            Ok(Some(session)) => session,
            Ok(None) | Err(cxp_core::Error::AccessRevoked) => return Ok(()),
            Err(e) => {
                tracing::error!(error = %e.log_detail(), "could not delete a console session");
                return Err(());
            }
        };
        if let Ok(refresh_token) = session.refresh_token(&i.keys.session_tokens) {
            self.revoke(&refresh_token).await;
        }
        Ok(())
    }

    /// Revoke a refresh token at the platform. Best effort, logged.
    async fn revoke(&self, refresh_token: &str) {
        let i = &self.inner;
        if let Err(e) = i.oauth.revoke(&i.settings.client_id, refresh_token).await {
            tracing::warn!(error = ?e, "could not revoke a console session's refresh token");
        }
    }

    /// Give back a refresh lease. Best effort: it lapses on its own.
    async fn release(&self, org: OrgId, id_hash: &[u8; 32], lease: Lease) {
        if let Err(e) = console_sessions::release_lease(&self.inner.db, org, id_hash, lease).await {
            tracing::warn!(error = %e.log_detail(), "could not release a console session's lease");
        }
    }

    /// Forget a session that will never work again. Best effort.
    async fn forget(&self, org: OrgId, id_hash: &[u8; 32]) {
        if let Err(e) = console_sessions::delete(&self.inner.db, org, id_hash).await {
            tracing::warn!(error = %e.log_detail(), "could not delete a dead console session");
        }
    }
}

fn db_failure(what: &str, e: cxp_core::Error) -> AuthFailure {
    if matches!(e, cxp_core::Error::AccessRevoked) {
        return AuthFailure::Revoked;
    }
    tracing::error!(error = %e.log_detail(), source = ?std::error::Error::source(&e),
        "could not {what}; answering 503");
    AuthFailure::Unavailable
}

/// When to treat an access token issued now, for `expires_in` seconds, as
/// expired. A token that lives at least twice [`REFRESH_SKEW_SECS`] is
/// refreshed that long before it expires. A shorter one is refreshed half way
/// through its life instead, so a request still gets a token with time left
/// and a short-lived token is not refreshed on every request: the stored
/// expiry is moved out by the difference.
fn access_expiry(expires_in: Option<i64>) -> DateTime<Utc> {
    let secs = expires_in.unwrap_or(DEFAULT_ACCESS_TTL_SECS).max(1);
    let effective = secs.max(secs / 2 + REFRESH_SKEW_SECS);
    Utc::now() + chrono::Duration::seconds(effective)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_claim_leaves_the_platform_uncalled() {
        assert!(fits_in_lease(Duration::ZERO));
        assert!(fits_in_lease(Duration::from_secs(15)));
        assert!(!fits_in_lease(Duration::from_millis(15_001)));
    }

    /// A panic in a refresh task must not leave its session's entry behind:
    /// every later request would join the dead task and answer 503.
    #[tokio::test]
    async fn a_panicking_refresh_task_still_frees_its_single_flight_entry() {
        let map: Inflight<u8> = Inflight::default();
        let key = [7u8; 32];
        map.lock().unwrap().insert(key, 0);
        map.lock().unwrap().insert([8u8; 32], 0);
        let entry = InflightEntry {
            map: Arc::clone(&map),
            key,
        };
        let outcome = tokio::spawn(async move {
            let _entry = entry;
            panic!("a refresh task panicked");
        })
        .await;
        assert!(outcome.unwrap_err().is_panic());
        let left: Vec<[u8; 32]> = map.lock().unwrap().keys().copied().collect();
        assert_eq!(
            left,
            vec![[8u8; 32]],
            "only the panicked task's entry is gone"
        );
    }

    #[test]
    fn every_console_scope_is_one_this_service_knows() {
        for scope in CONSOLE_SCOPES {
            assert!(cxp_core::scopes::KNOWN.contains(scope), "{scope}");
        }
    }

    const KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";

    #[test]
    fn the_two_subkeys_are_independent_and_neither_is_the_key() {
        let keys = ConsoleKeys::derive(KEY).unwrap();
        let raw = Cipher::from_base64_key(KEY).unwrap();
        let sealed = cxp_core::sealed::seal(&keys.session_tokens, b"t").unwrap();
        assert_eq!(
            cxp_core::sealed::open(&keys.session_tokens, &sealed).unwrap(),
            b"t"
        );
        assert!(cxp_core::sealed::open(&keys.oauth_flow, &sealed).is_err());
        assert!(cxp_core::sealed::open(&raw, &sealed).is_err());
        let sealed = cxp_core::sealed::seal(&keys.oauth_flow, b"f").unwrap();
        assert!(cxp_core::sealed::open(&keys.session_tokens, &sealed).is_err());
        // Deterministic: the same key derives the same subkeys.
        let again = ConsoleKeys::derive(KEY).unwrap();
        assert_eq!(
            cxp_core::sealed::open(&again.oauth_flow, &sealed).unwrap(),
            b"f"
        );
        // URL-safe base64 of the same bytes is the same key.
        let url_safe = URL_SAFE_NO_PAD.encode([7u8; 32]);
        let same = ConsoleKeys::derive(&url_safe).unwrap();
        assert!(cxp_core::sealed::open(&same.oauth_flow, &sealed).is_ok());
    }

    /// RFC 5869 appendix A.1: the HKDF-SHA256 the subkeys come from.
    #[test]
    fn hkdf_matches_the_rfc_vector() {
        let ikm = [0x0bu8; 22];
        let salt: Vec<u8> = (0x00..=0x0c).collect();
        let info: Vec<u8> = (0xf0..=0xf9).collect();
        let mut okm = [0u8; 42];
        Hkdf::<Sha256>::new(Some(&salt), &ikm)
            .expand(&info, &mut okm)
            .unwrap();
        let hex: String = okm.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    #[test]
    fn a_bad_key_is_refused_without_echoing_it() {
        for bad in ["", "not base64!!", &STANDARD.encode([1u8; 16])] {
            let err = ConsoleKeys::derive(bad).err().unwrap();
            assert!(err.contains("CXP_ENCRYPTION_KEY"), "{err}");
        }
    }

    #[test]
    fn a_short_lived_token_is_refreshed_half_way_not_on_every_request() {
        let now = Utc::now();
        let at = |secs| (access_expiry(Some(secs)) - now).num_seconds();
        // Long-lived: the real expiry, refreshed REFRESH_SKEW_SECS before it.
        assert!((3599..=3601).contains(&at(3600)));
        assert!((119..=121).contains(&at(120)));
        // 60 seconds: refreshed after 30, not at once.
        let sixty = at(60);
        assert!((89..=91).contains(&sixty), "{sixty}");
        assert!(sixty - REFRESH_SKEW_SECS >= 29);
        // Missing or nonsense: the default, or at least the skew.
        assert!((3599..=3601).contains(&(access_expiry(None) - now).num_seconds()));
        assert!(at(0) >= REFRESH_SKEW_SECS - 1);
    }
}
