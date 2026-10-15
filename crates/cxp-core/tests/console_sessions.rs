//! The console's sign-in sessions against a real Postgres: tokens sealed at
//! rest and opened only with the key, the cookie-to-org lookup, the pinned
//! read, rotation under the row lock, deletion, the `member.removed` cleanup,
//! and the expiry sweep in bounded batches under the maintenance marker.

mod common;

use chrono::{Duration, Utc};
use common::{db, org, otto_app_db, user};
use cxp_core::console_sessions::{self, NewSession};
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

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_locked_session_rotates_and_deletes(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let hash = new_session(&db, o, user(), Duration::days(1)).await;

    let (mut tx, s) = console_sessions::lock(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_refresh");
    let later = Utc::now() + Duration::days(30);
    console_sessions::rotate(
        &mut tx,
        &cipher(),
        &hash,
        "otto_at_next",
        "otto_rt_next",
        Utc::now() + Duration::minutes(60),
        later,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let s = console_sessions::load(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.access_token(&cipher()).unwrap(), "otto_at_next");
    assert_eq!(s.refresh_token(&cipher()).unwrap(), "otto_rt_next");
    assert!((s.expires_at - later).num_seconds().abs() < 2);

    let (mut tx, _) = console_sessions::lock(&db, o, &hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        console_sessions::delete_locked(&mut tx, &hash)
            .await
            .unwrap(),
        1
    );
    tx.commit().await.unwrap();
    assert!(console_sessions::lock(&db, o, &hash)
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
