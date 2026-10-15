//! The console's sign-in sessions against a real Postgres: tokens sealed at
//! rest and opened only with the key, the cookie-to-org lookup, the pinned
//! read, the refresh lease and compare-and-swap store, deletion, the `member.removed` cleanup,
//! and the expiry sweep in bounded batches under the maintenance marker.

mod common;

use chrono::{Duration, Utc};
use common::{db, org, otto_app_db, user};
use cxp_core::console_sessions::{self, Claim, NewSession, Refreshed};
use cxp_core::platform_events::{self, Outcome};
use cxp_core::Error;
use otto_tenant::crypto::Cipher;
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Db;
use serde_json::json;
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
    let Claim::Claimed(s) = console_sessions::claim_refresh(&db, o, &hash, soon, lease)
        .await
        .unwrap()
    else {
        panic!("not claimed");
    };
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    assert_eq!(s.generation, 0);
    assert!(s.refresh_lease_until.is_some());
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon, lease)
            .await
            .unwrap(),
        Claim::Leased
    ));
    // Once the lease lapses (its holder died), another may take it.
    let later = soon + Duration::seconds(16);
    let Claim::Claimed(again) = console_sessions::claim_refresh(&db, o, &hash, later, lease)
        .await
        .unwrap()
    else {
        panic!("a lapsed lease was not taken over");
    };
    assert_eq!(again.generation, 0);

    // The store wins once, at the claimed generation, and ends the lease.
    let expires = Utc::now() + Duration::days(30);
    let pair = |access, refresh| Refreshed {
        access_token: access,
        refresh_token: refresh,
        access_expires_at: Utc::now() + Duration::minutes(60),
        expires_at: expires,
    };
    assert!(console_sessions::store_refreshed(
        &db,
        &cipher(),
        o,
        &hash,
        0,
        &pair("otto_at_next", "otto_rt_next")
    )
    .await
    .unwrap());
    assert!(
        !console_sessions::store_refreshed(
            &db,
            &cipher(),
            o,
            &hash,
            0,
            &pair("otto_at_stale", "otto_rt_stale")
        )
        .await
        .unwrap(),
        "a second store at a spent generation"
    );
    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.access_token(&cipher()).unwrap(), "otto_at_next");
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_next");
    assert_eq!((s.generation, s.refresh_lease_until), (1, None));
    assert!((s.expires_at - expires).num_seconds().abs() < 2);

    // A released lease frees the session at once; at a stale generation it
    // changes nothing.
    let soon = Utc::now() + Duration::minutes(60);
    let Claim::Claimed(s) = console_sessions::claim_refresh(&db, o, &hash, soon, lease)
        .await
        .unwrap()
    else {
        panic!("not claimed");
    };
    console_sessions::release_lease(&db, o, &hash, s.generation + 1)
        .await
        .unwrap();
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon, lease)
            .await
            .unwrap(),
        Claim::Leased
    ));
    console_sessions::release_lease(&db, o, &hash, s.generation)
        .await
        .unwrap();
    assert!(matches!(
        console_sessions::claim_refresh(&db, o, &hash, soon, lease)
            .await
            .unwrap(),
        Claim::Claimed(_)
    ));

    // A refused refresh deletes only the generation it refused.
    assert_eq!(
        console_sessions::delete_generation(&db, o, &hash, 0)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        console_sessions::delete_generation(&db, o, &hash, 1)
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
        1,
        &pair("otto_at_late", "otto_rt_late")
    )
    .await
    .unwrap());
    assert!(console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .is_none());
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
