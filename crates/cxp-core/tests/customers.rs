//! The customer-data domain (customers, interactions, segments) against a real
//! Postgres, as a request runs it: through `Db::begin` as `cxp_app`.

mod common;

use common::{customers_both_ways, db, org, sqlstate};
use cxp_core::customers::{Customer, CustomerFilter, CustomerUpdate, CustomersExt, NewCustomer};
use cxp_core::interactions::{parse_json, InteractionsExt, NewInteraction, INTERACTION_TYPES};
use cxp_core::segments::{NewSegment, SegmentsExt};
use cxp_core::{Error, Page};
use savvagent_tenant::ids::OrgId;
use savvagent_tenant::Db;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn new(email: &str, name: &str) -> NewCustomer {
    NewCustomer {
        email: email.into(),
        name: Some(name.into()),
        ..Default::default()
    }
}

async fn add(db: &Db, org: OrgId, c: NewCustomer) -> Customer {
    let mut tx = db.begin(org).await.unwrap();
    let c = tx.create_customer(&c).await.unwrap();
    tx.commit().await.unwrap();
    c
}

/// Put a customer in a static segment with raw SQL: nothing in the domain API
/// writes members.
async fn member(tx: &mut savvagent_tenant::Tx<'_>, org: OrgId, segment: Uuid, customer: Uuid) {
    sqlx::query(
        "INSERT INTO segment_members (org_id, segment_id, customer_id) VALUES ($1, $2, $3)",
    )
    .bind(org)
    .bind(segment)
    .bind(customer)
    .execute(tx.conn())
    .await
    .unwrap();
}

/// Give a row an explicit creation time, `hours` after a fixed origin, so
/// ordering assertions cannot pass on ties (every row of one transaction
/// shares `now()`). `table` is a constant from these tests.
async fn stamp(tx: &mut savvagent_tenant::Tx<'_>, table: &str, id: Uuid, hours: i32) {
    sqlx::query(&format!(
        "UPDATE {table} SET created_at = timestamptz '2024-01-01 00:00+00' \
         + make_interval(hours => $2) WHERE id = $1"
    ))
    .bind(id)
    .bind(hours)
    .execute(tx.conn())
    .await
    .unwrap();
}

fn page(limit: i64, offset: i64) -> Page {
    Page::new(Some(limit), Some(offset))
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn customer_create_get_update_delete(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();

    let created = tx
        .create_customer(&NewCustomer {
            email: " ada@example.com ".into(),
            name: Some("Ada Lovelace".into()),
            company: Some("Analytical".into()),
            tags: vec!["vip".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(created.email, "ada@example.com");
    assert_eq!(created.metadata, json!({}));
    assert_eq!(created.tags, vec!["vip".to_string()]);

    assert_eq!(tx.get_customer(created.id).await.unwrap().id, created.id);

    let updated = tx
        .update_customer(
            created.id,
            &CustomerUpdate {
                phone: Some("555".into()),
                tags: Some(vec![]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.phone.as_deref(), Some("555"));
    assert!(updated.tags.is_empty());
    // Untouched fields stay.
    assert_eq!(updated.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(updated.company.as_deref(), Some("Analytical"));
    assert!(updated.updated_at >= created.updated_at);

    tx.delete_customer(created.id).await.unwrap();
    assert!(matches!(
        tx.get_customer(created.id).await,
        Err(Error::NotFound { kind: "customer" })
    ));
    assert!(matches!(
        tx.delete_customer(created.id).await,
        Err(Error::NotFound { kind: "customer" })
    ));
    assert!(matches!(
        tx.update_customer(created.id, &CustomerUpdate::default())
            .await,
        Err(Error::NotFound { kind: "customer" })
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn blank_email_is_invalid(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let e = tx.create_customer(&new("  ", "x")).await.unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn duplicate_email_conflicts_within_an_org_but_not_across_orgs(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    add(&db, a, new("dup@example.com", "One")).await;
    // Another org may have the same customer.
    add(&db, b, new("dup@example.com", "One")).await;

    let mut tx = db.begin(a).await.unwrap();
    let e = tx
        .create_customer(&new("dup@example.com", "Two"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), "conflict");
    assert!(!e.to_string().contains("customers_org_id_email_key"));
    drop(tx);

    let mut tx = db.begin(a).await.unwrap();
    let other = tx
        .create_customer(&new("other@example.com", "O"))
        .await
        .unwrap();
    let e = tx
        .update_customer(
            other.id,
            &CustomerUpdate {
                email: Some("dup@example.com".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), "conflict");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn list_pages_and_totals_and_filters(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let exp = Uuid::new_v4();
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query("INSERT INTO experiences (id, org_id, name) VALUES ($1, $2, 'Web')")
        .bind(exp)
        .bind(org)
        .execute(tx.conn())
        .await
        .unwrap();
    let mut ids = Vec::new();
    for i in 0..5 {
        let c = tx
            .create_customer(&new(&format!("c{i}@example.com"), &format!("Cust {i}")))
            .await
            .unwrap();
        stamp(&mut tx, "customers", c.id, i).await;
        ids.push(c.id);
    }
    sqlx::query("UPDATE customers SET experience_id = $3 WHERE org_id = $1 AND id = $2")
        .bind(org)
        .bind(ids[0])
        .bind(exp)
        .execute(tx.conn())
        .await
        .unwrap();

    let all = CustomerFilter::default();
    let (first, total) = tx.list_customers(&all, page(2, 0)).await.unwrap();
    assert_eq!((first.len(), total), (2, 5));
    let (second, total) = tx.list_customers(&all, page(2, 2)).await.unwrap();
    assert_eq!((second.len(), total), (2, 5));
    let (last, _) = tx.list_customers(&all, page(2, 4)).await.unwrap();
    assert_eq!(last.len(), 1);
    // Newest first, and pages continue where the last stopped.
    let order = |rows: &[Customer]| rows.iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(order(&first), vec![ids[4], ids[3]]);
    assert_eq!(order(&second), vec![ids[2], ids[1]]);
    assert_eq!(order(&last), vec![ids[0]]);

    let only = CustomerFilter {
        experience_id: Some(exp),
        ..Default::default()
    };
    let (rows, total) = tx.list_customers(&only, Page::default()).await.unwrap();
    assert_eq!((rows.len(), total), (1, 1));
    assert_eq!(rows[0].id, ids[0]);

    // A limit above the maximum is cut, never unbounded.
    let (_, total) = tx.list_customers(&all, page(1_000_000, 0)).await.unwrap();
    assert_eq!(total, 5);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn search_matches_email_and_name_and_escapes_wildcards(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    tx.create_customer(&new("grace@navy.example", "Grace Hopper"))
        .await
        .unwrap();
    tx.create_customer(&new("alan@bletchley.example", "Alan Turing"))
        .await
        .unwrap();
    tx.create_customer(&new("100%@example.com", "Percent"))
        .await
        .unwrap();

    let by_email = tx
        .search_customers(&common::permit().await, "NAVY", Page::default())
        .await
        .unwrap()
        .items;
    assert_eq!(by_email.len(), 1);
    assert_eq!(by_email[0].email, "grace@navy.example");

    let by_name = tx
        .search_customers(&common::permit().await, "turing", Page::default())
        .await
        .unwrap()
        .items;
    assert_eq!(by_name.len(), 1);

    // `%` is a literal here, not "match everything".
    let pct = tx
        .search_customers(&common::permit().await, "100%", Page::default())
        .await
        .unwrap()
        .items;
    assert_eq!(pct.len(), 1);
    let none = tx
        .search_customers(&common::permit().await, "zzzz", Page::default())
        .await
        .unwrap()
        .items;
    assert!(none.is_empty());

    let e = tx
        .search_customers(&common::permit().await, "   ", Page::default())
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");

    let one = tx
        .search_customers(&common::permit().await, "example", page(1, 0))
        .await
        .unwrap()
        .items;
    assert_eq!(one.len(), 1);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn resolve_by_uuid_email_and_name(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    let ada = add(&db, a, new("ada@example.com", "Ada Lovelace")).await;
    let ann1 = add(&db, a, new("ann1@example.com", "Ann Lee")).await;
    add(&db, a, new("ann2@example.com", "Ann Lee-Smith")).await;
    let foreign = add(&db, b, new("bea@example.com", "Bea")).await;

    let mut tx = db.begin(a).await.unwrap();
    assert_eq!(
        tx.resolve_customer(&ada.id.to_string()).await.unwrap(),
        ada.id
    );
    assert_eq!(
        tx.resolve_customer("ada@example.com").await.unwrap(),
        ada.id
    );
    assert_eq!(tx.resolve_customer("ada lovelace").await.unwrap(), ada.id);
    assert_eq!(tx.resolve_customer("Lovelace").await.unwrap(), ada.id);

    // An exact name wins over a looser match ("Ann Lee" also fits "Ann Lee-Smith").
    assert_eq!(tx.resolve_customer("Ann Lee").await.unwrap(), ann1.id);

    // A near miss is not a match: write tools resolve through here.
    assert!(matches!(
        tx.resolve_customer("Ada Lovelase").await,
        Err(Error::NotFound { kind: "customer" })
    ));
    assert_eq!(
        tx.resolve_customer("  ").await.unwrap_err().code(),
        "invalid_argument"
    );

    // Ambiguous: invalid, and it names nobody.
    let e = tx.resolve_customer("Ann").await.unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let shown = e.to_string();
    assert!(
        !shown.contains("ann1@") && !shown.contains("Lee"),
        "{shown}"
    );

    for missing in [
        "nobody@example.com",
        "Zed Nobody",
        &Uuid::new_v4().to_string(),
        // Another org's customer, by id, email, and name.
        &foreign.id.to_string(),
        "bea@example.com",
        "Bea",
    ] {
        assert!(
            matches!(
                tx.resolve_customer(missing).await,
                Err(Error::NotFound { kind: "customer" })
            ),
            "{missing}"
        );
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn interactions_record_list_and_journey(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    let c1 = tx
        .create_customer(&new("i1@example.com", "I1"))
        .await
        .unwrap();
    let c2 = tx
        .create_customer(&new("i2@example.com", "I2"))
        .await
        .unwrap();

    let mut ids = Vec::new();
    for (n, (c, t)) in [
        (&c1, "login"),
        (&c1, "purchase"),
        (&c1, "page_view"),
        (&c2, "login"),
    ]
    .into_iter()
    .enumerate()
    {
        let i = tx
            .record_interaction(&NewInteraction {
                customer_id: c.id,
                interaction_type: t.into(),
                metadata: None,
            })
            .await
            .unwrap();
        assert_eq!(i.interaction_type, t);
        assert_eq!(i.customer_id, c.id);
        assert_eq!(i.metadata, json!({}));
        stamp(&mut tx, "interactions", i.id, n as i32).await;
        ids.push(i.id);
    }
    let with_meta = tx
        .record_interaction(&NewInteraction {
            customer_id: c2.id,
            interaction_type: "feature_use".into(),
            metadata: Some(parse_json("metadata", r#"{"feature":"export"}"#).unwrap()),
        })
        .await
        .unwrap();
    assert_eq!(with_meta.metadata["feature"], "export");
    stamp(&mut tx, "interactions", with_meta.id, 4).await;

    let (all, total) = tx.list_interactions(page(2, 0)).await.unwrap();
    assert_eq!((all.len(), total), (2, 5));
    assert_eq!(
        all.iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![with_meta.id, ids[3]]
    );
    let (journey, total) = tx.get_journey(c1.id, Page::default()).await.unwrap();
    assert_eq!((journey.len(), total), (3, 3));
    assert!(journey.iter().all(|i| i.customer_id == c1.id));
    assert_eq!(
        journey.iter().map(|i| i.id).collect::<Vec<_>>(),
        vec![ids[2], ids[1], ids[0]]
    );
    let (paged, total) = tx.get_journey(c1.id, page(1, 1)).await.unwrap();
    assert_eq!((paged.len(), total), (1, 3));
    assert_eq!(paged[0].id, ids[1]);

    assert!(matches!(
        tx.get_journey(Uuid::new_v4(), Page::default()).await,
        Err(Error::NotFound { kind: "customer" })
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn bad_interaction_input_is_invalid_or_not_found(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let c = tx
        .create_customer(&new("x@example.com", "X"))
        .await
        .unwrap();

    let e = tx
        .record_interaction(&NewInteraction {
            customer_id: c.id,
            interaction_type: "teleport".into(),
            metadata: None,
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    assert!(e.to_string().contains("page_view"), "{e}");

    let e = tx
        .record_interaction(&NewInteraction {
            customer_id: Uuid::new_v4(),
            interaction_type: "login".into(),
            metadata: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(e, Error::NotFound { kind: "customer" }));

    assert_eq!(
        parse_json("metadata", "{nope").unwrap_err().code(),
        "invalid_argument"
    );
    // The transaction is still usable: nothing aborted it.
    tx.get_customer(c.id).await.unwrap();
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn dynamic_segments_evaluate_their_criteria(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    for (email, company, tags) in [
        ("a@x.example", Some("Acme"), vec!["vip"]),
        ("b@x.example", Some("Acme"), vec![]),
        ("c@x.example", Some("Other"), vec!["vip"]),
    ] {
        tx.create_customer(&NewCustomer {
            email: email.into(),
            company: company.map(Into::into),
            tags: tags.into_iter().map(Into::into).collect(),
            ..Default::default()
        })
        .await
        .unwrap();
    }

    let seg = |criteria| NewSegment {
        name: "s".into(),
        description: None,
        criteria,
        segment_type: None,
    };
    let both = tx
        .create_segment(&seg(json!({"tags_contain": "vip", "company_eq": "Acme"})))
        .await
        .unwrap();
    assert_eq!(both.segment_type, "dynamic");
    let (members, total) = tx
        .get_segment_members(both.id, Page::default())
        .await
        .unwrap();
    assert_eq!((members.len(), total), (1, 1));
    assert_eq!(members[0].email, "a@x.example");

    // A quote in a value is data, not SQL.
    let tricky = tx
        .create_segment(&seg(json!({"company_eq": "x' OR '1'='1"})))
        .await
        .unwrap();
    let (_, total) = tx
        .get_segment_members(tricky.id, Page::default())
        .await
        .unwrap();
    assert_eq!(total, 0);

    let future = tx
        .create_segment(&seg(json!({"created_after": "2999-01-01T00:00:00Z"})))
        .await
        .unwrap();
    assert_eq!(
        tx.get_segment_members(future.id, Page::default())
            .await
            .unwrap()
            .1,
        0
    );
    let past = tx
        .create_segment(&seg(json!({"created_before": "2999-01-01T00:00:00Z"})))
        .await
        .unwrap();
    assert_eq!(
        tx.get_segment_members(past.id, Page::default())
            .await
            .unwrap()
            .1,
        3
    );
    let (one, total) = tx.get_segment_members(past.id, page(1, 0)).await.unwrap();
    assert_eq!((one.len(), total), (1, 3));

    // Any timestamp Postgres reads is accepted: date-only, space-separated,
    // and with an offset.
    for (key, value, expect) in [
        ("created_after", "2000-01-01", 3),
        ("created_after", "2000-01-01 00:00:00", 3),
        ("created_after", "2000-01-01T00:00:00+02:00", 3),
        ("created_before", "2000-01-01", 0),
        ("created_before", "2999-12-31 23:59:59", 3),
    ] {
        let s = tx
            .create_segment(&seg(json!({ key: value })))
            .await
            .unwrap();
        let (_, total) = tx.get_segment_members(s.id, Page::default()).await.unwrap();
        assert_eq!(total, expect, "{key} {value}");
    }

    // Empty criteria match everyone.
    let everyone = tx.create_segment(&seg(json!({}))).await.unwrap();
    assert_eq!(
        tx.get_segment_members(everyone.id, Page::default())
            .await
            .unwrap()
            .1,
        3
    );

    let (listed, total) = tx.list_segments(page(2, 0)).await.unwrap();
    assert_eq!((listed.len(), total), (2, 10));
    assert_eq!(tx.get_segment(both.id).await.unwrap().id, both.id);
    assert!(matches!(
        tx.get_segment(Uuid::new_v4()).await,
        Err(Error::NotFound { kind: "segment" })
    ));
    assert!(matches!(
        tx.get_segment_members(Uuid::new_v4(), Page::default())
            .await,
        Err(Error::NotFound { kind: "segment" })
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn segment_input_is_validated(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let make = |name: &str, criteria, ty: Option<&str>| NewSegment {
        name: name.into(),
        description: None,
        criteria,
        segment_type: ty.map(Into::into),
    };
    for bad in [
        make("", json!({}), None),
        make("s", json!([]), None),
        make("s", json!("text"), Some("static")),
        make("s", json!({}), Some("weird")),
        make("s", json!({"colour": "red"}), None),
        make("s", json!({"company_eq": 5}), None),
        make("s", json!({"created_after": "not a date"}), None),
        make("s", json!({"created_before": "2024-13-45"}), None),
        make("s", json!({"created_after": "2024-01-01 Foo/Bar"}), None),
        make("s", json!({"created_after": "2024-01-01 +99:00"}), None),
    ] {
        let e = tx.create_segment(&bad).await.unwrap_err();
        assert_eq!(e.code(), "invalid_argument", "{bad:?}");
        assert!(!e.to_string().contains("timestamptz"), "{e}");
        assert!(!e.retriable() && !e.is_internal(), "{e:?}");
    }
    // The refusals did not abort the transaction.
    tx.create_segment(&make("ok", json!({}), None))
        .await
        .unwrap();

    // Unknown keys and non-string values name the keys that are allowed.
    for bad in [json!({"colour": "red"}), json!({"company_eq": 5})] {
        let e = tx.create_segment(&make("s", bad, None)).await.unwrap_err();
        for key in [
            "tags_contain",
            "company_eq",
            "created_after",
            "created_before",
        ] {
            assert!(e.to_string().contains(key), "{e}");
        }
    }
    assert_eq!(
        parse_json("criteria", "not json").unwrap_err().code(),
        "invalid_argument"
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn static_segments_list_their_members(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    let c1 = tx
        .create_customer(&new("s1@example.com", "S1"))
        .await
        .unwrap();
    let c2 = tx
        .create_customer(&new("s2@example.com", "S2"))
        .await
        .unwrap();
    tx.create_customer(&new("s3@example.com", "S3"))
        .await
        .unwrap();
    let seg = tx
        .create_segment(&NewSegment {
            name: "picked".into(),
            description: Some("hand-picked".into()),
            criteria: json!({}),
            segment_type: Some("static".into()),
        })
        .await
        .unwrap();
    assert_eq!(seg.description.as_deref(), Some("hand-picked"));
    assert_eq!(
        tx.get_segment_members(seg.id, Page::default())
            .await
            .unwrap()
            .1,
        0
    );

    member(&mut tx, org, seg.id, c1.id).await;
    member(&mut tx, org, seg.id, c2.id).await;
    let (members, total) = tx
        .get_segment_members(seg.id, Page::default())
        .await
        .unwrap();
    assert_eq!((members.len(), total), (2, 2));
    let (one, total) = tx.get_segment_members(seg.id, page(1, 1)).await.unwrap();
    assert_eq!((one.len(), total), (1, 2));

    // Deleting a customer drops their membership.
    tx.delete_customer(c2.id).await.unwrap();
    let (members, _) = tx
        .get_segment_members(seg.id, Page::default())
        .await
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, c1.id);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn customer_360_gathers_the_customers_recent_activity(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    let c = tx
        .create_customer(&new("360@example.com", "Three Sixty"))
        .await
        .unwrap();
    let other = tx
        .create_customer(&new("o@example.com", "Other"))
        .await
        .unwrap();

    for i in 0..12 {
        let rec = tx
            .record_interaction(&NewInteraction {
                customer_id: c.id,
                interaction_type: "login".into(),
                metadata: Some(json!({"n": i})),
            })
            .await
            .unwrap();
        stamp(&mut tx, "interactions", rec.id, i).await;
    }
    for (cust, subject, status, hours) in [
        (c.id, "Broken", "in_progress", 1),
        (c.id, "Slow", "open", 2),
        (other.id, "Not mine", "open", 3),
    ] {
        sqlx::query(
            "INSERT INTO tickets (org_id, customer_id, subject, status, priority, channel, \
                                  created_at) \
             VALUES ($1, $2, $3, $4::ticket_status, 'high', 'chat', \
                     timestamptz '2024-01-01 00:00+00' + make_interval(hours => $5))",
        )
        .bind(org)
        .bind(cust)
        .bind(subject)
        .bind(status)
        .bind(hours)
        .execute(tx.conn())
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO feedback (org_id, customer_id, feedback_type, subject, rating) \
         VALUES ($1, $2, 'praise', 'Nice', 5)",
    )
    .bind(org)
    .bind(c.id)
    .execute(tx.conn())
    .await
    .unwrap();

    let view = tx.customer_360(c.id).await.unwrap();
    assert_eq!(view.customer.id, c.id);
    // The newest ten of twelve, newest first.
    let ns: Vec<i64> = view
        .recent_interactions
        .iter()
        .map(|i| i.metadata["n"].as_i64().unwrap())
        .collect();
    assert_eq!(ns, (2..12).rev().collect::<Vec<_>>());
    let subjects: Vec<&str> = view
        .recent_tickets
        .iter()
        .map(|t| t.subject.as_str())
        .collect();
    assert_eq!(subjects, ["Slow", "Broken"]);
    let broken = &view.recent_tickets[1];
    assert_eq!(
        (
            broken.status.as_str(),
            broken.priority.as_str(),
            broken.channel.as_deref()
        ),
        ("in_progress", "high", Some("chat"))
    );
    assert_eq!(view.recent_feedback.len(), 1);
    assert_eq!(view.recent_feedback[0].feedback_type, "praise");
    assert_eq!(view.recent_feedback[0].rating, Some(5));

    // The wire shape is camelCase and carries no org id.
    let wire = serde_json::to_value(&view).unwrap();
    assert!(wire.get("recentTickets").is_some());
    assert!(!wire.to_string().contains("org_id") && !wire.to_string().contains("orgId"));

    assert!(matches!(
        tx.customer_360(Uuid::new_v4()).await,
        Err(Error::NotFound { kind: "customer" })
    ));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn one_org_cannot_see_or_touch_anothers_customer_data(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());

    // Org A's data.
    let mut tx = db.begin(a).await.unwrap();
    let cust = tx
        .create_customer(&new("a@example.com", "Alice"))
        .await
        .unwrap();
    let inter = tx
        .record_interaction(&NewInteraction {
            customer_id: cust.id,
            interaction_type: "login".into(),
            metadata: None,
        })
        .await
        .unwrap();
    let seg = tx
        .create_segment(&NewSegment {
            name: "A static".into(),
            description: None,
            criteria: json!({}),
            segment_type: Some("static".into()),
        })
        .await
        .unwrap();
    let dynamic = tx
        .create_segment(&NewSegment {
            name: "A all".into(),
            description: None,
            criteria: json!({}),
            segment_type: None,
        })
        .await
        .unwrap();
    member(&mut tx, a, seg.id, cust.id).await;
    tx.commit().await.unwrap();

    // Org B sees none of it.
    let mut tx = db.begin(b).await.unwrap();
    let all = CustomerFilter::default();
    let (rows, total) = tx.list_customers(&all, Page::default()).await.unwrap();
    assert!(rows.is_empty() && total == 0);
    assert!(tx
        .search_customers(&common::permit().await, "alice", Page::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(matches!(
        tx.get_customer(cust.id).await,
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        tx.customer_360(cust.id).await,
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        tx.update_customer(
            cust.id,
            &CustomerUpdate {
                name: Some("Mallory".into()),
                ..Default::default()
            }
        )
        .await,
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        tx.delete_customer(cust.id).await,
        Err(Error::NotFound { .. })
    ));

    let (rows, total) = tx.list_interactions(Page::default()).await.unwrap();
    assert!(rows.is_empty() && total == 0);
    assert!(matches!(
        tx.get_journey(cust.id, Page::default()).await,
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(
        tx.record_interaction(&NewInteraction {
            customer_id: cust.id,
            interaction_type: "login".into(),
            metadata: None,
        })
        .await,
        Err(Error::NotFound { kind: "customer" })
    ));

    let (rows, total) = tx.list_segments(Page::default()).await.unwrap();
    assert!(rows.is_empty() && total == 0);
    for id in [seg.id, dynamic.id] {
        assert!(matches!(
            tx.get_segment(id).await,
            Err(Error::NotFound { .. })
        ));
        assert!(matches!(
            tx.get_segment_members(id, Page::default()).await,
            Err(Error::NotFound { .. })
        ));
    }

    // An all-matching dynamic segment in B excludes A's customers.
    let b_dyn = tx
        .create_segment(&NewSegment {
            name: "B all".into(),
            description: None,
            criteria: json!({}),
            segment_type: None,
        })
        .await
        .unwrap();
    assert_eq!(
        tx.get_segment_members(b_dyn.id, Page::default())
            .await
            .unwrap()
            .1,
        0
    );

    // The database refuses the same references written as raw SQL: composite
    // foreign keys, not just the checks above.
    let refused = sqlx::query(
        "INSERT INTO interactions (org_id, customer_id, interaction_type) \
         VALUES ($1, $2, 'login')",
    )
    .bind(b)
    .bind(cust.id)
    .execute(tx.conn())
    .await
    .unwrap_err();
    assert_eq!(sqlstate(&refused), "23503");
    drop(tx);

    let mut tx = db.begin(b).await.unwrap();
    let b_seg2 = tx
        .create_segment(&NewSegment {
            name: "B2".into(),
            description: None,
            criteria: json!({}),
            segment_type: Some("static".into()),
        })
        .await
        .unwrap();
    let refused = sqlx::query(
        "INSERT INTO segment_members (org_id, segment_id, customer_id) VALUES ($1, $2, $3)",
    )
    .bind(b)
    .bind(b_seg2.id)
    .bind(cust.id)
    .execute(tx.conn())
    .await
    .unwrap_err();
    assert_eq!(sqlstate(&refused), "23503");
    drop(tx);

    // Org A's data is untouched by all of that.
    let mut tx = db.begin(a).await.unwrap();
    let c = tx.get_customer(cust.id).await.unwrap();
    assert_eq!(c.name.as_deref(), Some("Alice"));
    assert_eq!(
        tx.get_journey(cust.id, Page::default()).await.unwrap().0[0].id,
        inter.id
    );
    assert_eq!(
        tx.get_segment_members(seg.id, Page::default())
            .await
            .unwrap()
            .1,
        1
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn interaction_types_match_the_enum(pool: PgPool) {
    let (in_db,): (Vec<String>,) = sqlx::query_as(
        "SELECT array_agg(e::text ORDER BY e) FROM unnest(enum_range(NULL::interaction_type)) e",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut in_code: Vec<String> = INTERACTION_TYPES.iter().map(|t| t.to_string()).collect();
    in_code.sort();
    let mut in_db = in_db;
    in_db.sort();
    assert_eq!(in_code, in_db);
}

// ------------------------------------------------- search under RLS (#17)

/// Every query compared against the scan: substrings of every length
/// (one and two characters take the prefix path), whole emails, typos only
/// the fuzzy match finds, case and non-ASCII folding, LIKE wildcards and
/// the escape character, and queries matching nothing.
const QUERIES: &[&str] = &[
    "a",
    "Z",
    "ra",
    "é",
    "@",
    ".",
    "%",
    "_",
    "\\",
    "gr",
    "grace",
    "GRACE",
    "hopper",
    "grace hopper",
    "Grace Hoper",
    "Grase Hopper",
    "navy",
    "navy.example",
    "ce@na",
    "grace@navy.example",
    "alan@bletchley.example",
    "100%",
    "100%@",
    "a_b",
    "o'brien",
    "O'Brien",
    "José",
    "JOSÉ",
    "jose",
    "zoë",
    "Zoe",
    "müller",
    "MÜLLER",
    "straße",
    "STRASSE",
    "kelvin",
    "İstanbul",
    "istanbul",
    "3m",
    "x",
    "xyzzy",
    "  ",
    "e h",
    "lovelace",
    "Ada Lovelace",
    "Ada Lovelase",
    "Lin",
    "Linus",
    "torvalds",
    "TORVALDS",
    "smith",
    "Jon Smith",
    "john smyth",
    "li",
    "lee",
    "le",
    "wei",
    "王",
    "王小明",
    "小明",
    "mi",
];

async fn seed_search_corpus(tx: &mut savvagent_tenant::Tx<'_>) {
    let people: &[(&str, Option<&str>)] = &[
        ("grace@navy.example", Some("Grace Hopper")),
        ("alan@bletchley.example", Some("Alan Turing")),
        ("ada@example.com", Some("Ada Lovelace")),
        ("ada.l@example.org", Some("Ada Lovelase")),
        ("100%@example.com", Some("Percent")),
        ("a_b@example.com", Some("Under Score")),
        ("aXb@example.com", Some("Not Under Score")),
        ("back\\slash@example.com", Some("Back Slash")),
        ("obrien@example.com", Some("Conan O'Brien")),
        ("jose@example.es", Some("José Núñez")),
        ("zoe@example.fr", Some("Zoë Kravitz")),
        ("mueller@example.de", Some("Thomas Müller")),
        ("strasse@example.de", Some("Straße Weg")),
        ("kelvin@example.com", Some("\u{212A}elvin Temperature")),
        ("ist@example.tr", Some("İstanbul Office")),
        ("3m@example.com", Some("3M Company")),
        ("linus@kernel.example", Some("Linus Torvalds")),
        ("lin@example.com", Some("Lin")),
        ("john.smith@example.com", Some("John Smith")),
        ("jon.smyth@example.com", Some("Jon Smyth")),
        ("lee@example.com", Some("Lee")),
        ("wei@example.cn", Some("王小明")),
        ("noname@example.com", None),
        ("x@y.z", Some("X")),
    ];
    for (email, name) in people {
        tx.create_customer(&NewCustomer {
            email: (*email).into(),
            name: name.map(Into::into),
            ..Default::default()
        })
        .await
        .unwrap();
    }
}

/// The indexed search and the old full scan agree on `q` and `page`: the
/// same ids in the same order, and a total that is the exact count (none of
/// these corpora reaches the cap or a read bound).
async fn same_both_ways(tx: &mut savvagent_tenant::Tx<'_>, q: &str, page: Page, what: &str) {
    if let Some((indexed, scanned)) = customers_both_ways(tx, q, page).await {
        assert_eq!(indexed, scanned, "{what}, query {q:?}");
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn indexed_search_returns_exactly_what_the_scan_returns(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    for o in [a, b] {
        let mut tx = db.begin(o).await.unwrap();
        seed_search_corpus(&mut tx).await;
        tx.commit().await.unwrap();
    }
    let mut tx = db.begin(a).await.unwrap();
    // Twice over, so the prepared statement has gone to its generic plan
    // (after five executions) for most of the comparisons.
    for _ in 0..2 {
        for q in QUERIES {
            same_both_ways(&mut tx, q, Page::new(Some(200), None), "all").await;
        }
    }
    // Paging cuts the same ordered list, and counts the same total.
    let (indexed, scanned) = customers_both_ways(&mut tx, "example", page(3, 2))
        .await
        .unwrap();
    assert_eq!(indexed.ids.len(), 3);
    assert_eq!(indexed, scanned);
    // Org B's identical customers never appear.
    let all: Vec<Uuid> = tx
        .search_customers(&common::permit().await, "e", Page::new(Some(200), None))
        .await
        .unwrap()
        .items
        .iter()
        .map(|c| c.id)
        .collect();
    let mine: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM customers WHERE org_id = $1")
        .bind(a)
        .fetch_all(tx.conn())
        .await
        .unwrap();
    assert!(all.iter().all(|id| mine.contains(id)));
}

/// Fuzzy candidates are bounded with pg_trgm's own threshold, so a session
/// that changes it still gets what `%` gives, including the threshold of 0
/// at which every named customer is similar.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn indexed_search_follows_the_similarity_threshold(pool: PgPool) {
    let db = db(pool);
    let a = org();
    let mut tx = db.begin(a).await.unwrap();
    seed_search_corpus(&mut tx).await;
    for threshold in ["0", "0.1", "0.5", "0.9", "1"] {
        sqlx::query(&format!(
            "SET LOCAL pg_trgm.similarity_threshold = {threshold}"
        ))
        .execute(tx.conn())
        .await
        .unwrap();
        for q in ["Grace Hoper", "lin", "smith", "zzzz", "王小明", "%"] {
            let what = format!("threshold {threshold}");
            same_both_ways(&mut tx, q, Page::new(Some(200), None), &what).await;
        }
    }
}

/// Creating, renaming, and deleting a customer keep the search index in
/// step: the old name and email stop matching and the new ones match.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_search_index_follows_customer_writes(pool: PgPool) {
    let db = db(pool);
    let a = org();
    let mut tx = db.begin(a).await.unwrap();
    let c = tx
        .create_customer(&new("first@old.example", "Rosalind Franklin"))
        .await
        .unwrap();
    let ids = |found: Vec<Customer>| found.into_iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(
        ids(tx
            .search_customers(&common::permit().await, "rosalind", Page::default())
            .await
            .unwrap()
            .items),
        [c.id]
    );

    // Writing the same values again changes nothing (the trigger skips it).
    tx.update_customer(c.id, &CustomerUpdate::default())
        .await
        .unwrap();
    assert_eq!(
        ids(tx
            .search_customers(&common::permit().await, "old.example", Page::default())
            .await
            .unwrap()
            .items),
        [c.id]
    );

    tx.update_customer(
        c.id,
        &CustomerUpdate {
            email: Some("second@new.example".into()),
            name: Some("Barbara McClintock".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(tx
        .search_customers(&common::permit().await, "rosalind", Page::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(tx
        .search_customers(&common::permit().await, "old.example", Page::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        ids(tx
            .search_customers(&common::permit().await, "mcclintock", Page::default())
            .await
            .unwrap()
            .items),
        [c.id]
    );
    assert_eq!(
        ids(tx
            .search_customers(&common::permit().await, "Barbra McClintok", Page::default())
            .await
            .unwrap()
            .items),
        [c.id]
    );
    assert_eq!(
        ids(tx
            .search_customers(&common::permit().await, "new.ex", Page::default())
            .await
            .unwrap()
            .items),
        [c.id]
    );

    tx.delete_customer(c.id).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_search_grams")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert_eq!(left, 0, "deleting the customer leaves grams behind");
}

/// Writes that bypass this crate (the previous release, which knows nothing
/// of the index, or raw SQL) are indexed too: the triggers do it.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn raw_sql_writes_are_indexed(pool: PgPool) {
    let db = db(pool);
    let a = org();
    let mut tx = db.begin(a).await.unwrap();
    sqlx::query(
        "INSERT INTO customers (org_id, email, name) VALUES ($1, 'raw@sql.example', 'Raw Row')",
    )
    .bind(a)
    .execute(tx.conn())
    .await
    .unwrap();
    assert_eq!(
        tx.search_customers(&common::permit().await, "raw row", Page::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    sqlx::query("UPDATE customers SET name = 'Cooked Row' WHERE org_id = $1")
        .bind(a)
        .execute(tx.conn())
        .await
        .unwrap();
    assert!(tx
        .search_customers(&common::permit().await, "raw row", Page::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        tx.search_customers(&common::permit().await, "cooked", Page::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

/// Migration 0004's `search_mark_indexed` trigger sets `search_indexed` on
/// every insert and update, whatever the statement says, and the sync
/// triggers write the row's index rows with it: no ordinary write can leave
/// a row unindexed. That is why search reads no backlog of unindexed rows
/// (#34); only rows written before 0004, or loaded with triggers off (see
/// `the_rebuild_runbook_restores_lost_index_rows`), can be.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn rows_are_indexed_whatever_the_insert_says(pool: PgPool) {
    use cxp_core::knowledge::KnowledgeExt;
    let db = db(pool);
    let a = org();
    let mut tx = db.begin(a).await.unwrap();
    sqlx::query(
        "INSERT INTO customers (org_id, email, name, search_indexed) \
         VALUES ($1, 'flag@example.com', 'Flag Person', false)",
    )
    .bind(a)
    .execute(tx.conn())
    .await
    .unwrap();
    let indexed: bool = sqlx::query_scalar("SELECT search_indexed FROM customers")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert!(indexed, "the insert's false was overridden");
    let grams: i64 = sqlx::query_scalar("SELECT count(*) FROM customer_search_grams")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert!(grams > 0, "the customer has index rows");
    sqlx::query("UPDATE customers SET search_indexed = false WHERE org_id = $1")
        .bind(a)
        .execute(tx.conn())
        .await
        .unwrap();
    let indexed: bool = sqlx::query_scalar("SELECT search_indexed FROM customers")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert!(indexed, "the update's false was overridden");
    let found = tx
        .search_customers(&common::permit().await, "flag", Page::default())
        .await
        .unwrap();
    assert_eq!(
        (found.items.len(), found.total, found.total_capped),
        (1, 1, false)
    );

    sqlx::query(
        "INSERT INTO knowledge_articles (org_id, title, content, published, search_indexed) \
         VALUES ($1, 'Flag waving', 'How to wave a flag', true, false)",
    )
    .bind(a)
    .execute(tx.conn())
    .await
    .unwrap();
    let indexed: bool = sqlx::query_scalar("SELECT search_indexed FROM knowledge_articles")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert!(indexed, "the article insert's false was overridden");
    sqlx::query("UPDATE knowledge_articles SET search_indexed = false")
        .execute(tx.conn())
        .await
        .unwrap();
    let indexed: bool = sqlx::query_scalar("SELECT search_indexed FROM knowledge_articles")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert!(indexed, "the article update's false was overridden");
    let terms: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_search_terms")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert!(terms > 0, "the article has index rows");
    let found = tx
        .search_articles(&common::permit().await, "flag")
        .await
        .unwrap();
    assert_eq!(
        (found.items.len(), found.total, found.total_capped),
        (1, 1, false)
    );
}

/// The detection and rebuild runbook in docs/deploy/fly.md ("No backlog"),
/// statement for statement: rows whose index rows were lost (a load with
/// triggers off) still read `search_indexed = true`, so the runbook finds
/// them by their missing index rows and rebuilds them in two steps. If the
/// sync triggers' `WHEN` clauses change, this and the runbook change
/// together.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_rebuild_runbook_restores_lost_index_rows(pool: PgPool) {
    use cxp_core::knowledge::KnowledgeExt;
    const MISSING_CUSTOMERS: &str = "SELECT count(*) FROM customers c \
         WHERE NOT EXISTS (SELECT 1 FROM customer_search_grams g \
                           WHERE g.org_id = c.org_id AND g.customer_id = c.id)";
    const MISSING_ARTICLES: &str = "SELECT count(*) FROM knowledge_articles a \
         WHERE a.published AND (a.title <> '' OR a.search_tsv <> ''::tsvector) \
           AND NOT EXISTS (SELECT 1 FROM knowledge_search_terms t \
                           WHERE t.org_id = a.org_id AND t.article_id = a.id)";
    let su = pool.clone();
    let db = db(pool);
    let org = org();
    add(&db, org, new("lost@example.com", "Lost Person")).await;
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query(
        "INSERT INTO knowledge_articles (org_id, title, content, published) \
         VALUES ($1, 'Lost article', 'Some lost content', true)",
    )
    .bind(org)
    .execute(tx.conn())
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // What a load without the index tables leaves: the rows, flagged
    // indexed, with no index rows.
    sqlx::query("DELETE FROM customer_search_grams")
        .execute(&su)
        .await
        .unwrap();
    sqlx::query("DELETE FROM knowledge_search_terms")
        .execute(&su)
        .await
        .unwrap();
    let count = |sql: &'static str| {
        let su = su.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .fetch_one(&su)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        (
            count(MISSING_CUSTOMERS).await,
            count(MISSING_ARTICLES).await
        ),
        (1, 1)
    );
    let flagged: i64 = sqlx::query_scalar("SELECT count(*) FROM customers WHERE search_indexed")
        .fetch_one(&su)
        .await
        .unwrap();
    assert_eq!(flagged, 1, "the flag does not show the loss");

    let mut step1 = su.begin().await.unwrap();
    for stmt in [
        "SET LOCAL session_replication_role = replica",
        "UPDATE customers c SET search_indexed = false \
         WHERE NOT EXISTS (SELECT 1 FROM customer_search_grams g \
                           WHERE g.org_id = c.org_id AND g.customer_id = c.id)",
        "UPDATE knowledge_articles a SET search_indexed = false \
         WHERE a.published AND (a.title <> '' OR a.search_tsv <> ''::tsvector) \
           AND NOT EXISTS (SELECT 1 FROM knowledge_search_terms t \
                           WHERE t.org_id = a.org_id AND t.article_id = a.id)",
    ] {
        sqlx::query(stmt).execute(&mut *step1).await.unwrap();
    }
    step1.commit().await.unwrap();
    for stmt in [
        "UPDATE customers SET search_indexed = true WHERE NOT search_indexed",
        "UPDATE knowledge_articles SET search_indexed = true WHERE NOT search_indexed",
    ] {
        sqlx::query(stmt).execute(&su).await.unwrap();
    }

    assert_eq!(
        (
            count(MISSING_CUSTOMERS).await,
            count(MISSING_ARTICLES).await
        ),
        (0, 0)
    );
    let mut tx = db.begin(org).await.unwrap();
    let found = tx
        .search_customers(&common::permit().await, "lost", Page::default())
        .await
        .unwrap();
    assert_eq!((found.total, found.total_capped), (1, false));
    let found = tx
        .search_articles(&common::permit().await, "lost")
        .await
        .unwrap();
    assert_eq!((found.total, found.total_capped), (1, false));
}

/// A search that runs past its `statement_timeout` (here, waiting on a lock
/// on the index table that only the search statement reads) is
/// `query_too_broad`: not retriable, and saying nothing of statements or
/// cancellation, rather than an internal error.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_search_past_its_timeout_is_query_too_broad(pool: PgPool) {
    let su = pool.clone();
    let db = db(pool);
    let org = org();
    // Committed first: an uncommitted insert would hold a lock on the index
    // table, and the `LOCK TABLE` below would wait on it.
    add(&db, org, new("timeout@example.com", "Time Out")).await;

    let mut lock = su.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *lock)
        .await
        .unwrap();
    sqlx::query("LOCK TABLE customer_search_grams IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();

    let mut tx = db.begin(org).await.unwrap();
    let started = std::time::Instant::now();
    // Bounded, so a search that stopped setting its timeout fails here
    // instead of waiting on the lock for good.
    let e = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tx.search_customers(&common::permit().await, "anything", Page::default()),
    )
    .await
    .expect("the search waited past its own timeout")
    .unwrap_err();
    let waited = started.elapsed();
    drop(tx);
    lock.rollback().await.unwrap();

    assert_eq!(e.code(), "query_too_broad", "{e:?}");
    assert!(!e.retriable());
    let text = e.to_string();
    assert!(!text.contains("statement"), "{text}");
    assert!(!text.contains("cancel"), "{text}");
    assert!(
        waited >= std::time::Duration::from_millis(1900),
        "failed after {waited:?}, before the timeout"
    );
}

/// Many near-identical names, so plenty of candidates sit right at the
/// similarity threshold: a candidate bound that dropped one would show here.
/// Names and queries are drawn from a few syllables by a fixed generator.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn indexed_search_matches_the_scan_on_near_threshold_names(pool: PgPool) {
    const SYLLABLES: [&str; 8] = ["an", "na", "ber", "lin", "o", "kar", "el", "ta"];
    let mut state: u64 = 17;
    let mut word = |n: usize| {
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                SYLLABLES[(state >> 33) as usize % SYLLABLES.len()]
            })
            .collect::<String>()
    };
    let db = db(pool);
    let a = org();
    let mut tx = db.begin(a).await.unwrap();
    for i in 0..300 {
        let name = format!("{} {}", word(2 + i % 3), word(1 + i % 4));
        tx.create_customer(&new(&format!("c{i}@{}.example", word(2)), &name))
            .await
            .unwrap();
    }
    let queries: Vec<String> = (0..40)
        .map(|i| match i % 4 {
            0 => word(2),
            1 => format!("{} {}", word(2), word(2)),
            2 => word(4),
            _ => format!("{} {}", word(3), word(1)),
        })
        .collect();
    for threshold in ["0.2", "0.3", "0.45"] {
        sqlx::query(&format!(
            "SET LOCAL pg_trgm.similarity_threshold = {threshold}"
        ))
        .execute(tx.conn())
        .await
        .unwrap();
        for q in &queries {
            let what = format!("threshold {threshold}");
            same_both_ways(&mut tx, q, Page::new(Some(200), None), &what).await;
        }
    }
}

/// The ids of `found`, ordered as the pre-#17 search orders them: a page
/// past the cap must still be in that order.
async fn in_search_order(tx: &mut savvagent_tenant::Tx<'_>, q: &str, ids: &[Uuid]) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT id FROM customers WHERE org_id = $1 AND id = ANY ($4) \
         ORDER BY CASE WHEN email ILIKE $2 THEN 0 ELSE 1 END, \
                  similarity(name, $3) DESC NULLS LAST, created_at DESC, id",
    )
    .bind(tx.org())
    .bind(common::like_pattern(q))
    .bind(q)
    .bind(ids)
    .fetch_all(tx.conn())
    .await
    .unwrap()
}

/// Past the cap, search counts to it and says so: `total` is the cap,
/// `total_capped` is set, the page holds matches in the usual order, and a
/// page starting at or past the cap is empty. Exactly at the cap, and below
/// it, the count is exact and uncapped and the results are the scan's.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn search_counts_up_to_the_cap(pool: PgPool) {
    let db = db(pool);
    let a = org();
    let mut tx = db.begin(a).await.unwrap();
    // 1,100 customers at navy.example, exactly 1,000 named Zeta, and one
    // Unique Person: the triggers index every one.
    sqlx::query(
        "INSERT INTO customers (org_id, email, name, created_at) \
         SELECT $1, 'sailor' || n || '@navy.example', \
                (ARRAY['Grace','Alan','Ada','Linus'])[1 + n % 4] || ' Sailor' || n, \
                now() - make_interval(secs => n) \
         FROM generate_series(1, 1100) AS n",
    )
    .bind(a)
    .execute(tx.conn())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO customers (org_id, email, name) \
         SELECT $1, 'z' || n || '@example.org', 'Zeta Member' || n \
         FROM generate_series(1, 1000) AS n",
    )
    .bind(a)
    .execute(tx.conn())
    .await
    .unwrap();
    tx.create_customer(&new("unique@example.net", "Unique Person"))
        .await
        .unwrap();

    for q in ["navy", "navy.example", "na"] {
        let found = tx
            .search_customers(&common::permit().await, q, Page::default())
            .await
            .unwrap();
        assert_eq!((found.total, found.total_capped), (1000, true), "{q:?}");
        let ids: Vec<Uuid> = found.items.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 50, "{q:?}");
        assert_eq!(in_search_order(&mut tx, q, &ids).await, ids, "{q:?} order");
        let lower = q.to_lowercase();
        assert!(
            found.items.iter().all(|c| c.email.contains(&lower)
                || c.name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&lower)),
            "{q:?}: a non-match"
        );
    }
    // The page past the cap's edge is cut at it, and past it is empty.
    let edge = tx
        .search_customers(&common::permit().await, "navy", page(50, 990))
        .await
        .unwrap();
    assert_eq!(
        (edge.items.len(), edge.total, edge.total_capped),
        (10, 1000, true)
    );
    let past = tx
        .search_customers(&common::permit().await, "navy", page(50, 1000))
        .await
        .unwrap();
    assert_eq!(
        (past.items.len(), past.total, past.total_capped),
        (0, 1000, true)
    );

    // Exactly the cap: counted exactly, not capped.
    let (indexed, scanned) = customers_both_ways(&mut tx, "zeta", page(200, 0))
        .await
        .unwrap();
    assert_eq!((indexed.total, indexed.capped), (1000, false));
    assert_eq!(indexed, scanned);
    let (indexed, scanned) = customers_both_ways(&mut tx, "zeta", page(200, 900))
        .await
        .unwrap();
    assert_eq!(indexed.ids.len(), 100);
    assert_eq!(indexed, scanned);

    // Below it, among all these: exact.
    for q in [
        "unique",
        "Unique Persn",
        "sailor77@navy.example",
        "zeta member7",
    ] {
        let (indexed, scanned) = customers_both_ways(&mut tx, q, Page::default())
            .await
            .unwrap();
        assert!(!indexed.capped, "{q:?}");
        assert_eq!(indexed, scanned, "{q:?}");
    }
}

/// Queries and the searchable fields have length limits: probing grows with
/// a query's length, and a customer's index rows with its email and name.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn search_and_searchable_fields_are_length_limited(pool: PgPool) {
    use cxp_core::customers::{MAX_EMAIL_CHARS, MAX_NAME_CHARS};
    use cxp_core::search::MAX_QUERY_CHARS;
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();

    let e = tx
        .search_customers(
            &common::permit().await,
            &"q".repeat(MAX_QUERY_CHARS + 1),
            Page::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let found = tx
        .search_customers(
            &common::permit().await,
            &"q".repeat(MAX_QUERY_CHARS),
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((found.total, found.total_capped), (0, false));

    let at_limit = format!("{}@example.com", "e".repeat(MAX_EMAIL_CHARS - 12));
    assert_eq!(at_limit.chars().count(), MAX_EMAIL_CHARS);
    let c = tx
        .create_customer(&new(&at_limit, &"n".repeat(MAX_NAME_CHARS)))
        .await
        .unwrap();
    let too_long_email = format!("e{at_limit}");
    let too_long_name = "ñ".repeat(MAX_NAME_CHARS + 1);
    for bad in [
        new(&too_long_email, "Ok"),
        new("ok@example.com", &too_long_name),
    ] {
        let e = tx.create_customer(&bad).await.unwrap_err();
        assert_eq!(e.code(), "invalid_argument", "{e:?}");
    }
    for bad in [
        CustomerUpdate {
            email: Some(too_long_email.clone()),
            ..Default::default()
        },
        CustomerUpdate {
            name: Some(too_long_name.clone()),
            ..Default::default()
        },
    ] {
        let e = tx.update_customer(c.id, &bad).await.unwrap_err();
        assert_eq!(e.code(), "invalid_argument", "{e:?}");
    }
    let unchanged = tx.get_customer(c.id).await.unwrap();
    assert_eq!(unchanged.email, at_limit);
}

/// Rows written with the triggers off (a superuser load), as test setup for
/// states search must cope with.
async fn insert_untriggered(su: &PgPool, org: OrgId, sql: &str) {
    let mut t = su.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *t)
        .await
        .unwrap();
    sqlx::query(sql).bind(org).execute(&mut *t).await.unwrap();
    t.commit().await.unwrap();
}

/// What search found for `q` against what the scan finds: within the
/// contract, and `capped` as expected.
async fn assert_capped_within_contract(
    tx: &mut savvagent_tenant::Tx<'_>,
    q: &str,
) -> common::Searched {
    let (found, scanned) = customers_both_ways(tx, q, Page::default()).await.unwrap();
    let matches = common::customer_matches(tx, q).await;
    assert!(
        common::within_contract(&found, matches.len() as i64, &matches),
        "{q:?}: {found:?} against {} matches",
        matches.len()
    );
    assert!(found.total < 1000, "{q:?}: {found:?}");
    assert!(scanned.total > 0, "{q:?}: the setup has matches");
    found
}

/// A substring query whose rarest window has more index rows than search
/// reads: in an org of up to `NEWEST_READ` (20,000) customers the
/// newest-first read checks every customer, so the answer is still exact.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_cut_substring_read_is_exact_in_an_org_search_can_read_whole(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    // Every window of "abcdefgh" has at least 256 rows, so "sabcd" is the
    // rarest by its name, and it has 2,105 rows: more than the 2,000 read.
    sqlx::query(
        "INSERT INTO customers (org_id, email, name, created_at) \
         SELECT $1, 'abcdefg' || n || '@x.example', 'Zed Quux', now() - make_interval(secs => n) \
         FROM generate_series(1, 2100) AS n \
         UNION ALL \
         SELECT $1, 'bcdefgh' || n || '@y.example', 'Zed Quux', now() - make_interval(secs => n) \
         FROM generate_series(1, 300) AS n \
         UNION ALL \
         SELECT $1, 'abcdefgh' || n || '@z.example', 'Zed Quux', now() - make_interval(secs => n) \
         FROM generate_series(1, 5) AS n",
    )
    .bind(org)
    .execute(tx.conn())
    .await
    .unwrap();
    for q in ["abcdefgh", "abcdefgh3", "abcdefg2", "ABCDEFGH"] {
        let (found, scanned) = customers_both_ways(&mut tx, q, Page::default())
            .await
            .unwrap();
        assert_eq!(found, scanned, "{q:?}");
    }
    let (found, _) = customers_both_ways(&mut tx, "abcdefgh", Page::default())
        .await
        .unwrap();
    assert_eq!((found.total, found.capped), (5, false));
}

/// The same cut read in an org larger than search reads newest first: the
/// count is a lower bound, says so, and holds only matches.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_cut_substring_read_in_a_large_org_is_capped(pool: PgPool) {
    let su = pool.clone();
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query(
        "INSERT INTO customers (org_id, email, name, created_at) \
         SELECT $1, 'abcdefg' || n || '@x.example', 'Zed Quux', now() - make_interval(secs => n) \
         FROM generate_series(1, 2100) AS n \
         UNION ALL \
         SELECT $1, 'bcdefgh' || n || '@y.example', 'Zed Quux', now() - make_interval(secs => n) \
         FROM generate_series(1, 300) AS n \
         UNION ALL \
         SELECT $1, 'abcdefgh' || n || '@z.example', 'Zed Quux', \
                now() - interval '2 days' - make_interval(secs => n) \
         FROM generate_series(1, 5) AS n",
    )
    .bind(org)
    .execute(tx.conn())
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // 20,000 older customers that match nothing searched for here. Loaded
    // without their index rows (the query's grams are not among them, so
    // that changes only the org's size), the newest-first read stops among
    // them, before the five matches.
    insert_untriggered(
        &su,
        org,
        "INSERT INTO customers (org_id, email, name, created_at) \
         SELECT $1, 'old' || n || '@old.example', 'Old Filler', \
                now() - interval '1 day' - make_interval(secs => n) \
         FROM generate_series(1, 20000) AS n",
    )
    .await;
    let mut tx = db.begin(org).await.unwrap();
    let found = assert_capped_within_contract(&mut tx, "abcdefgh").await;
    assert!(found.capped);
}

/// A fuzzy query whose rarest trigrams' rows exceed the fuzzy read: capped,
/// a lower bound, and only matches.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_cut_fuzzy_read_is_capped(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    // 6,000 names sharing "  z", " zy", and "zyx" with "zyxw", too long to
    // be similar to it; three that are.
    sqlx::query(
        "INSERT INTO customers (org_id, email, name) \
         SELECT $1, 'f' || n || '@f.example', 'Zyx Abcdefgh' || n \
         FROM generate_series(1, 6000) AS n \
         UNION ALL \
         SELECT $1, 'w' || n || '@w.example', 'Zyxw' FROM generate_series(1, 3) AS n",
    )
    .bind(org)
    .execute(tx.conn())
    .await
    .unwrap();
    let found = assert_capped_within_contract(&mut tx, "zyxw").await;
    assert!(found.capped);
}

/// Random customers and queries (substrings, syllable runs, typos) against
/// the scan, at several pages: every answer is identical when uncapped, and
/// within the contract when capped.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn random_corpora_search_like_the_scan(pool: PgPool) {
    const SYLLABLES: [&str; 16] = [
        "an", "na", "ber", "lin", "o", "kar", "el", "ta", "gr", "ace", "hop", "per", "lee", "li",
        "zé", "ß",
    ];
    const DOMAINS: [&str; 3] = ["navy.example", "ex.com", "kar.org"];
    let db = db(pool);
    let org = org();
    let mut rng = common::Rng::new(0x17);
    let mut tx = db.begin(org).await.unwrap();
    let mut texts = Vec::new();
    for n in 0..400 {
        let word = |rng: &mut common::Rng| {
            let mut w = String::new();
            let n = 1 + rng.below(3);
            for _ in 0..n {
                w.push_str(SYLLABLES[rng.below(SYLLABLES.len())]);
            }
            w
        };
        let n_words = 1 + rng.below(3);
        let words: Vec<String> = (0..n_words).map(|_| word(&mut rng)).collect();
        let name = words
            .iter()
            .map(|w| {
                let mut c = w.chars();
                c.next()
                    .unwrap()
                    .to_uppercase()
                    .chain(c)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ");
        let local = word(&mut rng);
        let email = format!("{local}{n}@{}", DOMAINS[rng.below(DOMAINS.len())]);
        let name = (rng.below(10) > 0).then_some(name);
        let c = tx
            .create_customer(&NewCustomer {
                email: email.clone(),
                name: name.clone(),
                ..Default::default()
            })
            .await
            .unwrap();
        stamp(&mut tx, "customers", c.id, n % 37).await;
        texts.push(email);
        texts.extend(name);
    }
    // Queries search answered on the first page, and how many of those were
    // uncapped (exact): a corpus that capped most of them would prove little.
    let (mut run, mut uncapped) = (0, 0);
    for i in 0..250 {
        let text = texts[rng.below(texts.len())].clone();
        let mut q = match i % 4 {
            0 => rng.substring(&text, 8),
            1 => text,
            2 => {
                let n = 1 + rng.below(3);
                (0..n)
                    .map(|_| SYLLABLES[rng.below(SYLLABLES.len())])
                    .collect()
            }
            _ => rng.substring(&text, 12),
        };
        if i % 5 == 0 && q.chars().count() > 3 {
            // A typo: one character dropped.
            let at = rng.below(q.chars().count());
            q = q
                .chars()
                .enumerate()
                .filter(|(j, _)| *j != at)
                .map(|(_, c)| c)
                .collect();
        }
        for pg in [Page::default(), page(7, 3)] {
            let Some((found, scanned)) = customers_both_ways(&mut tx, &q, pg).await else {
                continue;
            };
            if pg == Page::default() {
                run += 1;
                uncapped += usize::from(!found.capped);
            }
            if found.capped {
                let matches = common::customer_matches(&mut tx, &q).await;
                assert!(
                    common::within_contract(&found, matches.len() as i64, &matches),
                    "{q:?}"
                );
            } else {
                assert_eq!(found, scanned, "query {q:?}, page {pg:?}");
            }
        }
    }
    // Measured when this bound was set (#34): 249 of 249 uncapped.
    // The corpus is fixed-seed, so the bound cannot flake: at least 95%.
    assert!(uncapped * 20 >= run * 19, "{uncapped} of {run} uncapped");
}
