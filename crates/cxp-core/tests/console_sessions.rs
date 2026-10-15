//! The console's sign-in sessions against a real Postgres: tokens sealed at
//! rest and opened only with the key, the cookie-to-org lookup, the pinned
//! read, the refresh lease and compare-and-swap store, deletion, the `member.removed` cleanup,
//! and the expiry sweep in bounded batches under the maintenance marker.

mod common;

use chrono::{Duration, Utc};
use common::{db, org, otto_app_db, user};
use cxp_core::console_sessions::{self, Claim, Lease, NewSession, Refreshed};
use cxp_core::platform_events::{self, Outcome};
use cxp_core::Error;
use otto_tenant::crypto::Cipher;
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

const KEY: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=";
const OTHER_KEY: &str = "CAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg=";

fn cipher() -> Cipher {
    Cipher::from_base64_key(KEY).unwrap()
}

/// A session for (`org`, `user`) under a fresh cookie, returning its hash.
async fn new_session(db: &Db, org: OrgId, user: UserId, expires_in: Duration) -> [u8; 32] {
    let cookie = Uuid::new_v4().to_string();
    let id_hash = console_sessions::hash_cookie(&cookie);
    console_sessions::create(
        db,
        &cipher(),
        &NewSession {
            id_hash,
            org_id: org,
            user_id: user,
            access_token: "otto_at_access",
            refresh_token: "otto_rt_refresh",
            access_expires_at: Utc::now() + Duration::minutes(60),
            expires_at: Utc::now() + expires_in,
        },
    )
    .await
    .unwrap();
    id_hash
}

#[test]
fn the_cookie_hash_is_sha256() {
    // SHA-256("abc"), FIPS 180-2 appendix B.1.
    assert_eq!(
        console_sessions::hash_cookie("abc")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_session_is_stored_sealed_and_found_by_its_cookie(pool: PgPool) {
    let db = db(pool.clone());
    let (o, u) = (org(), user());
    let hash = new_session(&db, o, u, Duration::days(30)).await;

    // Nothing in the row is a token in the clear.
    let (access, refresh): (Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT access_token_enc, refresh_token_enc FROM console_sessions WHERE id_hash = $1",
    )
    .bind(&hash[..])
    .fetch_one(&pool)
    .await
    .unwrap();
    for sealed in [&access, &refresh] {
        assert!(!sealed
            .windows(7)
            .any(|w| w == b"otto_at" || w == b"otto_rt"));
    }

    assert_eq!(console_sessions::org_of(&db, &hash).await.unwrap(), Some(o));
    assert_eq!(
        console_sessions::org_of(&db, &[0u8; 32]).await.unwrap(),
        None
    );

    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((s.org_id, s.user_id), (o, u));
    assert_eq!(s.access_token(&cipher()).unwrap(), "otto_at_access");
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    assert!(!s.needs_refresh(Utc::now()));
    assert!(s.needs_refresh(s.access_expires_at - Duration::seconds(30)));
    // The wrong key opens nothing; Debug shows no token.
    let other = Cipher::from_base64_key(OTHER_KEY).unwrap();
    assert!(s.access_token(&other).is_err());
    let shown = format!("{s:?}");
    assert!(
        !shown.contains("otto_at") && !shown.contains("enc"),
        "{shown}"
    );

    // Pinned to another org, the row does not exist.
    assert!(console_sessions::load(&db, org(), &hash)
        .await
        .unwrap()
        .is_none());
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_expired_session_is_not_found(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let hash = new_session(&db, o, user(), Duration::seconds(-1)).await;
    assert_eq!(console_sessions::org_of(&db, &hash).await.unwrap(), None);
    assert!(console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .is_none());
}

/// A sign-in that completes while the platform's cleanup runs must not leave
/// a session behind it.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn no_session_is_created_for_a_deleted_org_or_a_removed_member(pool: PgPool) {
    let db = db(pool);
    let (gone, live, leaver) = (org(), org(), user());
    let deleted = otto_resource::webhook::body(
        Uuid::new_v4(),
        "org.deleted",
        Utc::now(),
        &json!({"org_id": gone.as_uuid()}),
    );
    let removed = otto_resource::webhook::body(
        Uuid::new_v4(),
        "member.removed",
        Utc::now(),
        &json!({"org_id": live.as_uuid(), "user_id": leaver.as_uuid()}),
    );
    for body in [deleted, removed] {
        let event = otto_resource::webhook::parse(&body).unwrap();
        platform_events::apply(&db, &event).await.unwrap();
    }
    for (o, u) in [(gone, user()), (live, leaver)] {
        let err = console_sessions::create(
            &db,
            &cipher(),
            &NewSession {
                id_hash: console_sessions::hash_cookie("c"),
                org_id: o,
                user_id: u,
                access_token: "a",
                refresh_token: "r",
                access_expires_at: Utc::now(),
                expires_at: Utc::now() + Duration::days(1),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::AccessRevoked), "{err:?}");
    }
}

/// The refresh protocol's database half: a lease claimed in one short
/// transaction, a compare-and-swap store in another, and nothing held in
/// between.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_refresh_is_leased_then_stored_as_a_compare_and_swap(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let hash = new_session(&db, o, user(), Duration::days(1)).await;
    let lease = Duration::seconds(15);

    // A fresh token needs no refresh: nothing is claimed.
    let now = Utc::now();
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, now, lease)
            .await
            .unwrap(),
        Claim::Fresh(_)
    ));

    // About to expire: the first claim wins, a second sees the live lease.
    let soon = now + Duration::minutes(60);
    let Claim::Claimed(s, claimed) = console_sessions::claim_refresh(&db, o, &hash, soon, lease)
        .await
        .unwrap()
    else {
        panic!("not claimed");
    };
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    assert_eq!(claimed.generation, 0);
    assert_eq!(s.refresh_lease_until, Some(claimed.until));
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon, lease)
            .await
            .unwrap(),
        Claim::Leased
    ));
    // The caller's clock does not decide when a lease lapses: a caller whose
    // clock runs a minute ahead still sees it live.
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon + Duration::minutes(1), lease)
            .await
            .unwrap(),
        Claim::Leased
    ));

    // The store wins once, at the claimed lease, and ends the lease.
    let expires = Utc::now() + Duration::days(30);
    assert!(console_sessions::store_refreshed(
        &db,
        &cipher(),
        o,
        &hash,
        claimed,
        &pair("otto_at_next", "otto_rt_next", expires)
    )
    .await
    .unwrap());
    assert!(
        !console_sessions::store_refreshed(
            &db,
            &cipher(),
            o,
            &hash,
            claimed,
            &pair("otto_at_stale", "otto_rt_stale", expires)
        )
        .await
        .unwrap(),
        "a second store at a spent lease"
    );
    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.access_token(&cipher()).unwrap(), "otto_at_next");
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_next");
    assert_eq!((s.generation, s.refresh_lease_until), (1, None));
    assert!((s.expires_at - expires).num_seconds().abs() < 2);

    // A released lease frees the session at once.
    let soon = Utc::now() + Duration::minutes(60);
    let Claim::Claimed(_, claimed) = console_sessions::claim_refresh(&db, o, &hash, soon, lease)
        .await
        .unwrap()
    else {
        panic!("not claimed");
    };
    console_sessions::release_lease(&db, o, &hash, claimed)
        .await
        .unwrap();
    let Claim::Claimed(_, current) = console_sessions::claim_refresh(&db, o, &hash, soon, lease)
        .await
        .unwrap()
    else {
        panic!("a released lease was not claimable");
    };

    // A refused refresh deletes only the generation it refused.
    let older = Lease {
        generation: 0,
        until: current.until,
    };
    assert_eq!(
        console_sessions::delete_generation(&db, o, &hash, older)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        console_sessions::delete_generation(&db, o, &hash, current)
            .await
            .unwrap(),
        1
    );
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon, lease)
            .await
            .unwrap(),
        Claim::Gone
    ));
    // A store after the session went (a sign-out raced the platform call)
    // resurrects nothing.
    assert!(!console_sessions::store_refreshed(
        &db,
        &cipher(),
        o,
        &hash,
        claimed,
        &pair("otto_at_late", "otto_rt_late", expires)
    )
    .await
    .unwrap());
    assert!(console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .is_none());
}

fn pair<'a>(access: &'a str, refresh: &'a str, expires: chrono::DateTime<Utc>) -> Refreshed<'a> {
    Refreshed {
        access_token: access,
        refresh_token: refresh,
        access_expires_at: Utc::now() + Duration::minutes(60),
        expires_at: expires,
    }
}

/// A holder that died (or stalled) mid-refresh: once its lease lapses on the
/// database's clock, another caller takes the session over. The stale holder,
/// when it comes back, can neither release the newer claim nor store over it;
/// only the newer holder's store lands.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_lapsed_lease_is_taken_over_and_its_stale_holder_changes_nothing(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let hash = new_session(&db, o, user(), Duration::days(1)).await;
    let soon = Utc::now() + Duration::minutes(60);
    let short = Duration::milliseconds(200);

    let Claim::Claimed(_, stale) = console_sessions::claim_refresh(&db, o, &hash, soon, short)
        .await
        .unwrap()
    else {
        panic!("not claimed");
    };
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon, short)
            .await
            .unwrap(),
        Claim::Leased
    ));
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    // Lapsed: taken over, at the same generation, under a new lease.
    let Claim::Claimed(s, newer) =
        console_sessions::claim_refresh(&db, o, &hash, soon, Duration::seconds(15))
            .await
            .unwrap()
    else {
        panic!("a lapsed lease was not taken over");
    };
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    assert_eq!(newer.generation, stale.generation);
    assert!(newer.until > stale.until);

    // The stale holder's release frees nothing: the newer claim stands.
    console_sessions::release_lease(&db, o, &hash, stale)
        .await
        .unwrap();
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon, Duration::seconds(15))
            .await
            .unwrap(),
        Claim::Leased
    ));
    // Nor does its store land, though the generation still matches.
    let expires = Utc::now() + Duration::days(30);
    assert!(!console_sessions::store_refreshed(
        &db,
        &cipher(),
        o,
        &hash,
        stale,
        &pair("otto_at_stale", "otto_rt_stale", expires)
    )
    .await
    .unwrap());
    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    assert_eq!(
        (s.generation, s.refresh_lease_until),
        (0, Some(newer.until))
    );

    // The newer holder's store does.
    assert!(console_sessions::store_refreshed(
        &db,
        &cipher(),
        o,
        &hash,
        newer,
        &pair("otto_at_next", "otto_rt_next", expires)
    )
    .await
    .unwrap());
    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_next");
    assert_eq!((s.generation, s.refresh_lease_until), (1, None));
}

/// A lease that lapsed with no one claiming it since still matches: a slow
/// holder's store keeps the pair the platform issued instead of losing it.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_lapsed_but_unclaimed_lease_still_stores(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let hash = new_session(&db, o, user(), Duration::days(1)).await;
    let soon = Utc::now() + Duration::minutes(60);
    let Claim::Claimed(_, lease) =
        console_sessions::claim_refresh(&db, o, &hash, soon, Duration::milliseconds(100))
            .await
            .unwrap()
    else {
        panic!("not claimed");
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(console_sessions::store_refreshed(
        &db,
        &cipher(),
        o,
        &hash,
        lease,
        &pair(
            "otto_at_next",
            "otto_rt_next",
            Utc::now() + Duration::days(30)
        )
    )
    .await
    .unwrap());
}

/// A holder whose lease lapsed and was claimed anew at the same generation
/// cannot delete the session when the platform refuses its (stale) refresh
/// token: the newer holder's claim stands, and only that holder's lease
/// deletes it.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_stale_holder_never_deletes_a_session_claimed_anew(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let hash = new_session(&db, o, user(), Duration::days(1)).await;
    let soon = Utc::now() + Duration::minutes(60);
    let Claim::Claimed(_, stale) =
        console_sessions::claim_refresh(&db, o, &hash, soon, Duration::milliseconds(200))
            .await
            .unwrap()
    else {
        panic!("not claimed");
    };
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let Claim::Claimed(_, newer) =
        console_sessions::claim_refresh(&db, o, &hash, soon, Duration::seconds(15))
            .await
            .unwrap()
    else {
        panic!("a lapsed lease was not taken over");
    };
    assert_eq!(newer.generation, stale.generation);

    assert_eq!(
        console_sessions::delete_generation(&db, o, &hash, stale)
            .await
            .unwrap(),
        0,
        "a stale holder deleted a session claimed anew"
    );
    assert!(console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        console_sessions::delete_generation(&db, o, &hash, newer)
            .await
            .unwrap(),
        1
    );
    assert!(console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .is_none());
}

/// A fresh session on `db`, claimed for a refresh: its hash and the lease.
async fn claimed_session(db: &Db, o: OrgId) -> ([u8; 32], Lease) {
    let hash = new_session(db, o, user(), Duration::days(1)).await;
    let soon = Utc::now() + Duration::minutes(60);
    let Claim::Claimed(_, lease) =
        console_sessions::claim_refresh(db, o, &hash, soon, Duration::seconds(45))
            .await
            .unwrap()
    else {
        panic!("not claimed");
    };
    (hash, lease)
}

/// Longer than the store's own bound, short enough that a test which waited
/// on its lock forever fails rather than hangs.
const NO_LONGER_THAN: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether `result` failed because a database-side bound cut a wait off:
/// `55P03` (`lock_timeout`) or `57014` (`statement_timeout`), and not for
/// some unrelated reason.
fn timed_out<T: std::fmt::Debug>(result: &Result<T, Error>) -> bool {
    let db = match result {
        Err(Error::Database(e)) | Err(Error::Tenant(otto_tenant::Error::Db(e))) => e,
        _ => return false,
    };
    matches!(
        db.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("55P03" | "57014")
    )
}

/// The store runs inside the lease holder's `STORE_ALLOWANCE`; a row lock
/// held elsewhere must make it fail in time, not wait it out, and the failed
/// transaction changes nothing and leaves the pool usable.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_store_behind_a_held_row_lock_times_out(pool: PgPool) {
    let db = db(pool.clone());
    let o = org();
    let (hash, lease) = claimed_session(&db, o).await;

    // The pool's superuser bypasses RLS.
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM console_sessions WHERE id_hash = $1 FOR UPDATE")
        .bind(&hash[..])
        .execute(&mut *holder)
        .await
        .unwrap();

    let expires = Utc::now() + Duration::days(30);
    let stored = tokio::time::timeout(
        NO_LONGER_THAN,
        console_sessions::store_refreshed(
            &db,
            &cipher(),
            o,
            &hash,
            lease,
            &pair("otto_at_next", "otto_rt_next", expires),
        ),
    )
    .await
    .expect("the store waited out the row lock");
    assert!(timed_out(&stored), "{stored:?}");
    holder.rollback().await.unwrap();

    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    assert_eq!(
        (s.generation, s.refresh_lease_until),
        (0, Some(lease.until))
    );
}

/// Giving a lease back is bounded the same way.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_release_behind_a_held_row_lock_times_out(pool: PgPool) {
    let db = db(pool.clone());
    let o = org();
    let (hash, lease) = claimed_session(&db, o).await;

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM console_sessions WHERE id_hash = $1 FOR UPDATE")
        .bind(&hash[..])
        .execute(&mut *holder)
        .await
        .unwrap();

    let released = tokio::time::timeout(
        NO_LONGER_THAN,
        console_sessions::release_lease(&db, o, &hash, lease),
    )
    .await
    .expect("the release waited out the row lock");
    assert!(timed_out(&released), "{released:?}");
    holder.rollback().await.unwrap();

    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.refresh_lease_until, Some(lease.until));
}

/// The org's lifecycle lock, held exclusively (as `org.deleted`,
/// `member.removed`, and `team.deleted` hold it), makes the store time out
/// rather than wait; once it is released the same store lands.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_store_behind_the_lifecycle_lock_times_out(pool: PgPool) {
    let db = db(pool.clone());
    let o = org();
    let (hash, lease) = claimed_session(&db, o).await;

    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2::text))")
        .bind(platform_events::LIFECYCLE_LOCK)
        .bind(o)
        .execute(&mut *holder)
        .await
        .unwrap();

    let expires = Utc::now() + Duration::days(30);
    let next = pair("otto_at_next", "otto_rt_next", expires);
    let stored = tokio::time::timeout(
        NO_LONGER_THAN,
        console_sessions::store_refreshed(&db, &cipher(), o, &hash, lease, &next),
    )
    .await
    .expect("the store waited out the lifecycle lock");
    assert!(timed_out(&stored), "{stored:?}");
    holder.rollback().await.unwrap();

    assert!(
        console_sessions::store_refreshed(&db, &cipher(), o, &hash, lease, &next)
            .await
            .unwrap()
    );
}

/// The timeouts are the transaction's own (`SET LOCAL`): a connection that
/// ran a store goes back to the pool with the server's defaults, so no other
/// query on it inherits them.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_store_timeouts_do_not_outlive_its_transaction(pool: PgPool) {
    let one = Db::from_pool(
        PgPoolOptions::new()
            .max_connections(1)
            .connect_with(pool.connect_options().as_ref().clone())
            .await
            .unwrap(),
    );
    // As at boot: this resolves the tenant-role probe up front, which
    // `Db::begin` would otherwise run on a second connection while holding
    // the only one.
    one.verify_tenant_isolation().await.expect("isolation");
    let o = org();
    let (hash, lease) = claimed_session(&one, o).await;
    assert!(console_sessions::store_refreshed(
        &one,
        &cipher(),
        o,
        &hash,
        lease,
        &pair(
            "otto_at_next",
            "otto_rt_next",
            Utc::now() + Duration::days(30)
        )
    )
    .await
    .unwrap());
    for setting in ["lock_timeout", "statement_timeout"] {
        let value: String = sqlx::query_scalar(&format!("SHOW {setting}"))
            .fetch_one(one.pool())
            .await
            .unwrap();
        assert_eq!(value, "0", "{setting}");
    }
    one.pool().close().await;
}

/// Sign-out deletes and hands back what the row held, once.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_session_is_taken_once(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let hash = new_session(&db, o, user(), Duration::days(1)).await;
    let taken = console_sessions::take(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(taken.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    assert!(console_sessions::take(&db, o, &hash)
        .await
        .unwrap()
        .is_none());
    assert_eq!(console_sessions::delete(&db, o, &hash).await.unwrap(), 0);
}

/// `member.removed` signs the user out of that org, and only of that org,
/// on its first delivery. A replay must not sign out a member who has since
/// been re-invited and signed in again.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn member_removed_deletes_that_members_sessions_once(pool: PgPool) {
    let db = db(pool);
    let (o, elsewhere, leaver, stays) = (org(), org(), user(), user());
    let gone = new_session(&db, o, leaver, Duration::days(1)).await;
    let kept_other_org = new_session(&db, elsewhere, leaver, Duration::days(1)).await;
    let kept_other_user = new_session(&db, o, stays, Duration::days(1)).await;

    let body = otto_resource::webhook::body(
        Uuid::new_v4(),
        "member.removed",
        Utc::now(),
        &json!({"org_id": o.as_uuid(), "user_id": leaver.as_uuid()}),
    );
    let event = otto_resource::webhook::parse(&body).unwrap();
    assert!(matches!(
        platform_events::apply(&db, &event).await.unwrap(),
        Outcome::Applied { .. }
    ));
    assert_eq!(console_sessions::org_of(&db, &gone).await.unwrap(), None);
    assert_eq!(
        console_sessions::org_of(&db, &kept_other_org)
            .await
            .unwrap(),
        Some(elsewhere)
    );
    assert_eq!(
        console_sessions::org_of(&db, &kept_other_user)
            .await
            .unwrap(),
        Some(o)
    );

    // Re-invited (the tombstone lapses) and signed in again; then the
    // original delivery is replayed.
    sqlx::query("DELETE FROM removed_members")
        .execute(db.pool())
        .await
        .unwrap();
    let again = new_session(&db, o, leaver, Duration::days(1)).await;
    assert_eq!(
        platform_events::apply(&db, &event).await.unwrap(),
        Outcome::Duplicate
    );
    assert_eq!(
        console_sessions::org_of(&db, &again).await.unwrap(),
        Some(o)
    );
}

/// The sweep deletes only expired sessions, across orgs, in batches, as
/// `otto_app` (the policies admit it only under the maintenance marker).
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_sweep_deletes_expired_sessions_in_batches(pool: PgPool) {
    let admin = db(pool.clone());
    let (a, b) = (org(), org());
    let live = new_session(&admin, a, user(), Duration::days(1)).await;
    let expired = console_sessions::SWEEP_BATCH + 3;
    for o in [a, b] {
        sqlx::query(
            "INSERT INTO console_sessions (id_hash, org_id, user_id, access_token_enc, \
             refresh_token_enc, access_expires_at, expires_at) \
             SELECT sha256(gen_random_uuid()::text::bytea), $1, gen_random_uuid(), '\\x00', \
                    '\\x00', now(), now() - interval '1 second' \
             FROM generate_series(1, $2)",
        )
        .bind(o)
        .bind(expired)
        .execute(&pool)
        .await
        .unwrap();
    }

    let app_db = otto_app_db(&pool).await;
    let swept = console_sessions::sweep_expired(&app_db).await.unwrap();
    assert_eq!(swept, 2 * expired as u64);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM console_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 1);
    assert_eq!(
        console_sessions::org_of(&admin, &live).await.unwrap(),
        Some(a)
    );

    // And the hourly retention sweep runs it.
    sqlx::query("UPDATE console_sessions SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(platform_events::sweep(&app_db, 30).await.unwrap(), 1);
    app_db.pool().close().await;
}
