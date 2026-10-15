//! The feedback domain against a real Postgres: feedback, surveys, survey
//! responses, NPS/CSAT inputs, and isolation between two orgs.

mod common;

use common::{db, org};
use cxp_core::analysis::{calculate_csat, calculate_nps};
use cxp_core::feedback::{
    FeedbackExt, FeedbackFilter, NewFeedback, NewSurvey, SurveyFilter, SurveyUpdate,
};
use cxp_core::{Error, Page};
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn customer(db: &Db, org: OrgId) -> Uuid {
    let id = Uuid::new_v4();
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query("INSERT INTO customers (id, org_id, email, name) VALUES ($1, $2, $3, 'Ada')")
        .bind(id)
        .bind(org)
        .bind(format!("{id}@example.com"))
        .execute(tx.conn())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

/// An experience/application/page/component chain, returned in that order.
async fn catalog(db: &Db, org: OrgId) -> [Uuid; 4] {
    let ids: [Uuid; 4] = std::array::from_fn(|_| Uuid::new_v4());
    let mut tx = db.begin(org).await.unwrap();
    for sql in [
        "INSERT INTO experiences (id, org_id, name) VALUES ($2, $1, 'Checkout')",
        "INSERT INTO applications (id, org_id, experience_id, name) VALUES ($3, $1, $2, 'Web')",
        "INSERT INTO pages (id, org_id, application_id, name) VALUES ($4, $1, $3, 'Cart')",
        "INSERT INTO components (id, org_id, page_id, name) VALUES ($5, $1, $4, 'Pay')",
    ] {
        sqlx::query(&format!(
            "WITH _p AS (SELECT $1::uuid, $2::uuid, $3::uuid, $4::uuid, $5::uuid) {sql}"
        ))
        .bind(org)
        .bind(ids[0])
        .bind(ids[1])
        .bind(ids[2])
        .bind(ids[3])
        .execute(tx.conn())
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    ids
}

fn fb(customer: Uuid, t: &str, rating: Option<i32>) -> NewFeedback {
    NewFeedback {
        customer_id: customer,
        feedback_type: t.into(),
        rating,
        ..Default::default()
    }
}

fn survey(title: &str) -> NewSurvey {
    NewSurvey {
        title: title.into(),
        questions: json!([{"id": "q1", "text": "How likely?"}]),
        ..Default::default()
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn submit_feedback_stores_a_complete_row_with_catalog_ids(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    let [exp, app, page, comp] = catalog(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    let f = tx
        .submit_feedback(&NewFeedback {
            customer_id: c,
            feedback_type: "bug_report".into(),
            subject: Some("Broken".into()),
            body: Some("It broke".into()),
            rating: Some(2),
            experience_id: Some(exp),
            application_id: Some(app),
            page_id: Some(page),
            component_id: Some(comp),
        })
        .await
        .unwrap();
    assert_eq!(f.customer_id, c);
    assert_eq!(f.feedback_type, "bug_report");
    assert_eq!(f.subject.as_deref(), Some("Broken"));
    assert_eq!(f.body.as_deref(), Some("It broke"));
    assert_eq!(f.rating, Some(2));
    assert_eq!(f.metadata, json!({}));
    assert_eq!(
        (f.experience_id, f.application_id, f.page_id, f.component_id),
        (Some(exp), Some(app), Some(page), Some(comp))
    );
    // Never exposes the org.
    assert!(!serde_json::to_string(&f).unwrap().contains("orgId"));
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn bad_feedback_type_and_rating_are_invalid(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    for bad in [
        fb(c, "love_letter", None),
        fb(c, "praise", Some(0)),
        fb(c, "praise", Some(6)),
    ] {
        let err = tx.submit_feedback(&bad).await.unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{err:?}");
        assert_eq!(err.code(), "invalid_argument");
    }
    let err = tx
        .list_feedback(
            &FeedbackFilter {
                feedback_type: Some("nope".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Invalid(_)));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn unknown_customer_or_catalog_row_is_not_found(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    for (new, kind) in [
        (fb(Uuid::new_v4(), "praise", None), "customer"),
        (
            NewFeedback {
                experience_id: Some(Uuid::new_v4()),
                ..fb(c, "praise", None)
            },
            "experience",
        ),
        (
            NewFeedback {
                application_id: Some(Uuid::new_v4()),
                ..fb(c, "praise", None)
            },
            "application",
        ),
        (
            NewFeedback {
                component_id: Some(Uuid::new_v4()),
                ..fb(c, "praise", None)
            },
            "component",
        ),
    ] {
        let mut tx = db.begin(org).await.unwrap();
        match tx.submit_feedback(&new).await.unwrap_err() {
            Error::NotFound { kind: k } => assert_eq!(k, kind),
            other => panic!("{other:?}"),
        }
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn list_feedback_filters_pages_and_counts(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let (a, b) = (customer(&db, org).await, customer(&db, org).await);
    let [exp, app, page, comp] = catalog(&db, org).await;
    // Each in its own transaction so created_at really increases.
    let mut ids = Vec::new();
    for (c, t) in [
        (a, "praise"),
        (a, "bug_report"),
        (b, "praise"),
        (b, "complaint"),
        (b, "praise"),
    ] {
        let mut tx = db.begin(org).await.unwrap();
        ids.push(tx.submit_feedback(&fb(c, t, None)).await.unwrap().id);
        tx.commit().await.unwrap();
    }
    let mut tx = db.begin(org).await.unwrap();
    let newest = tx
        .submit_feedback(&NewFeedback {
            experience_id: Some(exp),
            application_id: Some(app),
            page_id: Some(page),
            component_id: Some(comp),
            ..fb(a, "suggestion", None)
        })
        .await
        .unwrap()
        .id;

    let (all, total) = tx
        .list_feedback(&FeedbackFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!((all.len(), total), (6, 6));
    assert_eq!(all[0].id, newest, "newest first");
    assert_eq!(all[1].id, *ids.last().unwrap());
    assert_eq!(all[5].id, ids[0]);

    // Paging: total stays the full match count.
    let (p, total) = tx
        .list_feedback(&FeedbackFilter::default(), Page::new(Some(2), Some(4)))
        .await
        .unwrap();
    assert_eq!((p.len(), total), (2, 6));
    let (p, _) = tx
        .list_feedback(&FeedbackFilter::default(), Page::new(Some(2), Some(40)))
        .await
        .unwrap();
    assert!(p.is_empty());

    let (r, total) = tx
        .list_feedback(
            &FeedbackFilter {
                customer_id: Some(b),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((r.len(), total), (3, 3));
    let (r, total) = tx
        .list_feedback(
            &FeedbackFilter {
                feedback_type: Some("praise".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((r.len(), total), (3, 3));
    let (r, total) = tx
        .list_feedback(
            &FeedbackFilter {
                customer_id: Some(b),
                feedback_type: Some("praise".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((r.len(), total), (2, 2));
    for f in [
        FeedbackFilter {
            experience_id: Some(exp),
            ..Default::default()
        },
        FeedbackFilter {
            application_id: Some(app),
            ..Default::default()
        },
        FeedbackFilter {
            page_id: Some(page),
            ..Default::default()
        },
        FeedbackFilter {
            component_id: Some(comp),
            ..Default::default()
        },
    ] {
        let (r, total) = tx.list_feedback(&f, Page::default()).await.unwrap();
        assert_eq!((r.len(), total), (1, 1));
        assert_eq!(r[0].feedback_type, "suggestion");
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn csat_counts_aggregate_in_sql_and_match_the_pure_math(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    let empty = tx.feedback_csat_counts(None).await.unwrap();
    assert_eq!(empty.csat(), None);
    assert_eq!(empty.total_feedback, 0);

    let ratings = [5, 4, 3, 1, 4, 2];
    for r in ratings {
        tx.submit_feedback(&fb(c, "praise", Some(r))).await.unwrap();
    }
    tx.submit_feedback(&fb(c, "praise", None)).await.unwrap();
    tx.submit_feedback(&fb(c, "complaint", Some(1)))
        .await
        .unwrap();

    let praise = tx.feedback_csat_counts(Some("praise")).await.unwrap();
    assert_eq!(praise.total_feedback, 7);
    assert_eq!(praise.rated_feedback, 6);
    assert_eq!((praise.satisfied, praise.unsatisfied), (3, 3));
    assert_eq!(praise.csat(), calculate_csat(&ratings));

    let all = tx.feedback_csat_counts(None).await.unwrap();
    assert_eq!((all.total_feedback, all.rated_feedback), (8, 7));
    let mut with_complaint = ratings.to_vec();
    with_complaint.push(1);
    assert_eq!(all.csat(), calculate_csat(&with_complaint));
    assert!(matches!(
        tx.feedback_csat_counts(Some("nope")).await.unwrap_err(),
        Error::Invalid(_)
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn survey_crud_filters_and_totals(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let [exp, app, page, comp] = catalog(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();

    let s = tx
        .create_survey(&NewSurvey {
            description: Some("d".into()),
            experience_id: Some(exp),
            application_id: Some(app),
            page_id: Some(page),
            component_id: Some(comp),
            ..survey("NPS")
        })
        .await
        .unwrap();
    assert!(s.active);
    assert_eq!(s.title, "NPS");
    assert_eq!(s.description.as_deref(), Some("d"));
    assert_eq!(s.component_id, Some(comp));
    // now() is the transaction's start, so "newer" needs its own transaction.
    tx.commit().await.unwrap();
    let mut tx = db.begin(org).await.unwrap();
    let plain = tx.create_survey(&survey("Plain")).await.unwrap();

    assert_eq!(tx.get_survey(s.id).await.unwrap().id, s.id);
    assert!(matches!(
        tx.get_survey(Uuid::new_v4()).await.unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));

    let (all, total) = tx
        .list_surveys(&SurveyFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!((all.len(), total), (2, 2));
    assert_eq!(all[0].id, plain.id, "newest first");
    let (p, total) = tx
        .list_surveys(&SurveyFilter::default(), Page::new(Some(1), Some(1)))
        .await
        .unwrap();
    assert_eq!((p.len(), total), (1, 2));
    assert_eq!(p[0].id, s.id);
    for f in [
        SurveyFilter {
            experience_id: Some(exp),
            ..Default::default()
        },
        SurveyFilter {
            application_id: Some(app),
            ..Default::default()
        },
        SurveyFilter {
            page_id: Some(page),
            ..Default::default()
        },
        SurveyFilter {
            component_id: Some(comp),
            ..Default::default()
        },
    ] {
        let (r, total) = tx.list_surveys(&f, Page::default()).await.unwrap();
        assert_eq!((r.len(), total), (1, 1));
        assert_eq!(r[0].id, s.id);
    }

    // Partial update keeps what it does not name.
    let u = tx
        .update_survey(
            s.id,
            &SurveyUpdate {
                title: Some("NPS Q4".into()),
                active: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(u.title, "NPS Q4");
    assert!(!u.active);
    assert_eq!(u.description.as_deref(), Some("d"));
    assert_eq!(u.questions, s.questions);
    assert!(u.updated_at >= s.updated_at);
    let u = tx
        .update_survey(
            s.id,
            &SurveyUpdate {
                questions: Some(json!([])),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(u.questions, json!([]));
    assert_eq!(u.title, "NPS Q4");
    assert!(matches!(
        tx.update_survey(Uuid::new_v4(), &SurveyUpdate::default())
            .await
            .unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));

    tx.delete_survey(s.id).await.unwrap();
    assert!(matches!(
        tx.delete_survey(s.id).await.unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn bad_survey_input_is_invalid(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    for bad in [
        NewSurvey {
            questions: json!({"not": "an array"}),
            ..survey("x")
        },
        NewSurvey {
            questions: json!(null),
            ..survey("x")
        },
        survey("  "),
    ] {
        assert!(matches!(
            tx.create_survey(&bad).await.unwrap_err(),
            Error::Invalid(_)
        ));
    }
    let s = tx.create_survey(&survey("ok")).await.unwrap();
    assert!(matches!(
        tx.update_survey(
            s.id,
            &SurveyUpdate {
                questions: Some(json!("str")),
                ..Default::default()
            }
        )
        .await
        .unwrap_err(),
        Error::Invalid(_)
    ));
    assert!(matches!(
        tx.submit_survey_response(s.id, c, &json!("nope"))
            .await
            .unwrap_err(),
        Error::Invalid(_)
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn create_survey_with_unknown_catalog_ref_is_not_found(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let err = tx
        .create_survey(&NewSurvey {
            page_id: Some(Uuid::new_v4()),
            ..survey("x")
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound { kind: "page" }), "{err:?}");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn send_then_submit_and_list_responses(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    let s = tx.create_survey(&survey("NPS")).await.unwrap();

    let sent = tx.send_survey(s.id, c).await.unwrap();
    assert_eq!((sent.survey_id, sent.customer_id), (s.id, c));
    assert_eq!(sent.answers, json!({}));
    assert!(sent.completed_at.is_none());

    let answers = json!([{"questionId": "q1", "value": 9}]);
    tx.commit().await.unwrap();
    let mut tx = db.begin(org).await.unwrap();
    let done = tx.submit_survey_response(s.id, c, &answers).await.unwrap();
    assert_eq!(done.answers, answers);
    assert!(done.completed_at.is_some());
    assert_eq!((done.survey_id, done.customer_id), (s.id, c));
    assert_ne!(done.id, sent.id);

    let (r, total) = tx
        .list_survey_responses(s.id, Page::default())
        .await
        .unwrap();
    assert_eq!((r.len(), total), (2, 2));
    assert_eq!(r[0].id, done.id, "newest first");
    let (r, total) = tx
        .list_survey_responses(s.id, Page::new(Some(1), Some(1)))
        .await
        .unwrap();
    assert_eq!((r.len(), total), (1, 2));
    assert!(matches!(
        tx.list_survey_responses(Uuid::new_v4(), Page::default())
            .await
            .unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));

    // Responses go with their survey.
    tx.delete_survey(s.id).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM survey_responses")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn submit_survey_response_is_one_complete_write_or_nothing(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    let s = {
        let mut tx = db.begin(org).await.unwrap();
        let s = tx.create_survey(&survey("NPS")).await.unwrap();
        tx.commit().await.unwrap();
        s
    };

    // A refused submission (unknown customer) leaves no half-made row.
    let mut tx = db.begin(org).await.unwrap();
    let err = tx
        .submit_survey_response(s.id, Uuid::new_v4(), &json!({"score": 9}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::NotFound { kind: "customer" }),
        "{err:?}"
    );
    tx.rollback().await.unwrap();
    let mut tx = db.begin(org).await.unwrap();
    assert!(matches!(
        tx.submit_survey_response(Uuid::new_v4(), c, &json!({"score": 9}))
            .await
            .unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
    tx.rollback().await.unwrap();
    let mut tx = db.begin(org).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM survey_responses")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert_eq!(n, 0);
    tx.rollback().await.unwrap();

    // A good one is committed whole: answers and completed_at together, and
    // exactly one row (no separate "sent" row).
    let mut tx = db.begin(org).await.unwrap();
    let r = tx
        .submit_survey_response(s.id, c, &json!({"score": 9}))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin(org).await.unwrap();
    let rows: Vec<(
        Uuid,
        serde_json::Value,
        Option<chrono::DateTime<chrono::Utc>>,
    )> = sqlx::query_as("SELECT id, answers, completed_at FROM survey_responses")
        .fetch_all(tx.conn())
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, r.id);
    assert_eq!(rows[0].1, json!({"score": 9}));
    assert!(rows[0].2.is_some());
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn nps_counts_aggregate_in_sql_and_match_the_pure_math(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let c = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    let s = tx.create_survey(&survey("NPS")).await.unwrap();
    let empty = tx.survey_nps_counts(s.id).await.unwrap();
    assert_eq!(empty.nps(), None);
    assert_eq!(empty.total_responses, 0);

    // Both stored shapes, numeric strings, and unscorable answers.
    let scored = [
        (json!([{"questionId": "q1", "value": 10}]), 10),
        (json!([{"questionId": "q1", "value": "9"}]), 9),
        (json!({"score": 8}), 8),
        (json!({"score": 7}), 7),
        (json!({"score": 6}), 6),
        (json!([{"value": 0}]), 0),
        (json!({"score": 2}), 2),
    ];
    for (a, _) in &scored {
        tx.submit_survey_response(s.id, c, a).await.unwrap();
    }
    for a in [
        json!({}),
        json!([]),
        json!({"score": "high"}),
        json!({"score": 9.5}),
        json!({"score": "9"}),
        json!({"score": 11}),
        json!([{"value": null}]),
    ] {
        tx.submit_survey_response(s.id, c, &a).await.unwrap();
    }
    tx.send_survey(s.id, c).await.unwrap();

    let n = tx.survey_nps_counts(s.id).await.unwrap();
    let scores: Vec<i32> = scored.iter().map(|(_, s)| *s).collect();
    assert_eq!(n.total_responses, 15);
    assert_eq!(n.scored_responses, 7);
    assert_eq!((n.promoters, n.passives, n.detractors), (2, 2, 3));
    assert_eq!(n.nps(), calculate_nps(&scores));

    assert!(matches!(
        tx.survey_nps_counts(Uuid::new_v4()).await.unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn another_org_cannot_see_or_change_this_orgs_data(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    let ca = customer(&db, a).await;
    let cb = customer(&db, b).await;
    let [exp_a, ..] = catalog(&db, a).await;

    let mut tx = db.begin(a).await.unwrap();
    let fa = tx
        .submit_feedback(&fb(ca, "praise", Some(5)))
        .await
        .unwrap();
    let sa = tx.create_survey(&survey("A's")).await.unwrap();
    let ra = tx
        .submit_survey_response(sa.id, ca, &json!({"score": 10}))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin(b).await.unwrap();
    // Reads see nothing of A's.
    let (rows, total) = tx
        .list_feedback(&FeedbackFilter::default(), Page::default())
        .await
        .unwrap();
    assert!(rows.is_empty() && total == 0, "{fa:?}");
    let (rows, total) = tx
        .list_feedback(
            &FeedbackFilter {
                customer_id: Some(ca),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert!(rows.is_empty() && total == 0);
    let (rows, total) = tx
        .list_surveys(&SurveyFilter::default(), Page::default())
        .await
        .unwrap();
    assert!(rows.is_empty() && total == 0);
    assert!(matches!(
        tx.get_survey(sa.id).await.unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
    assert!(matches!(
        tx.list_survey_responses(sa.id, Page::default())
            .await
            .unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
    assert!(matches!(
        tx.survey_nps_counts(sa.id).await.unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
    let csat = tx.feedback_csat_counts(None).await.unwrap();
    assert_eq!((csat.total_feedback, csat.rated_feedback), (0, 0));

    // Writes against A's survey fail.
    assert!(matches!(
        tx.update_survey(
            sa.id,
            &SurveyUpdate {
                title: Some("hijacked".into()),
                ..Default::default()
            }
        )
        .await
        .unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
    assert!(matches!(
        tx.delete_survey(sa.id).await.unwrap_err(),
        Error::NotFound { kind: "survey" }
    ));
    tx.rollback().await.unwrap();

    // Each refusal aborts its transaction, so each gets a fresh one.
    let refused = |what: &'static str, kind: &'static str| {
        let db = db_clone(&db);
        async move {
            let mut tx = db.begin(b).await.unwrap();
            let err = match what {
                "response_to_a_survey" => tx
                    .submit_survey_response(sa.id, cb, &json!({"score": 1}))
                    .await
                    .map(|_| ())
                    .unwrap_err(),
                "send_to_a_survey" => tx.send_survey(sa.id, cb).await.map(|_| ()).unwrap_err(),
                "response_for_a_customer" => {
                    let s = tx.create_survey(&survey("B's")).await.unwrap();
                    tx.submit_survey_response(s.id, ca, &json!({"score": 1}))
                        .await
                        .map(|_| ())
                        .unwrap_err()
                }
                "feedback_for_a_customer" => tx
                    .submit_feedback(&fb(ca, "praise", None))
                    .await
                    .map(|_| ())
                    .unwrap_err(),
                "feedback_with_an_experience" => tx
                    .submit_feedback(&NewFeedback {
                        experience_id: Some(exp_a),
                        ..fb(cb, "praise", None)
                    })
                    .await
                    .map(|_| ())
                    .unwrap_err(),
                "survey_with_an_experience" => tx
                    .create_survey(&NewSurvey {
                        experience_id: Some(exp_a),
                        ..survey("x")
                    })
                    .await
                    .map(|_| ())
                    .unwrap_err(),
                _ => unreachable!(),
            };
            match err {
                Error::NotFound { kind: k } => assert_eq!(k, kind, "{what}"),
                other => panic!("{what}: {other:?}"),
            }
        }
    };
    refused("response_to_a_survey", "survey").await;
    refused("send_to_a_survey", "survey").await;
    refused("response_for_a_customer", "customer").await;
    refused("feedback_for_a_customer", "customer").await;
    refused("feedback_with_an_experience", "experience").await;
    refused("survey_with_an_experience", "experience").await;

    // A's data is untouched.
    let mut tx = db.begin(a).await.unwrap();
    let s = tx.get_survey(sa.id).await.unwrap();
    assert_eq!(s.title, "A's");
    let (rows, total) = tx
        .list_survey_responses(sa.id, Page::default())
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (1, 1));
    assert_eq!(rows[0].id, ra.id);
    let (_, total) = tx
        .list_feedback(&FeedbackFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!(total, 1);
}

fn db_clone(db: &Db) -> Db {
    cxp_core::tenant_db(db.pool().clone())
}
