//! The customer-data domain (customers, interactions, segments) against a real
//! Postgres, as a request runs it: through `Db::begin` as `otto_app`.

mod common;

use common::{db, org, sqlstate};
use cxp_core::customers::{Customer, CustomerFilter, CustomerUpdate, CustomersExt, NewCustomer};
use cxp_core::interactions::{parse_json, InteractionsExt, NewInteraction};
use cxp_core::segments::{NewSegment, SegmentsExt};
use cxp_core::{Error, Page};
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
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
    let seen: std::collections::HashSet<_> = first
        .iter()
        .chain(&second)
        .chain(&last)
        .map(|c| c.id)
        .collect();
    assert_eq!(seen.len(), 5);
    // Newest first.
    assert!(first[0].created_at >= first[1].created_at);

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

    let by_email = tx.search_customers("NAVY", Page::default()).await.unwrap();
    assert_eq!(by_email.len(), 1);
    assert_eq!(by_email[0].email, "grace@navy.example");

    let by_name = tx
        .search_customers("turing", Page::default())
        .await
        .unwrap();
    assert_eq!(by_name.len(), 1);

    // `%` is a literal here, not "match everything".
    let pct = tx.search_customers("100%", Page::default()).await.unwrap();
    assert_eq!(pct.len(), 1);
    let none = tx.search_customers("zzzz", Page::default()).await.unwrap();
    assert!(none.is_empty());

    let e = tx
        .search_customers("   ", Page::default())
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");

    let one = tx.search_customers("example", page(1, 0)).await.unwrap();
    assert_eq!(one.len(), 1);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn resolve_by_uuid_email_and_name(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    let ada = add(&db, a, new("ada@example.com", "Ada Lovelace")).await;
    add(&db, a, new("ann1@example.com", "Ann Lee")).await;
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
    let ann = tx.resolve_customer("Ann Lee").await.unwrap();
    assert_ne!(ann, ada.id);

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

    for (c, t) in [
        (&c1, "login"),
        (&c1, "purchase"),
        (&c1, "page_view"),
        (&c2, "login"),
    ] {
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

    let (all, total) = tx.list_interactions(page(2, 0)).await.unwrap();
    assert_eq!((all.len(), total), (2, 5));
    let (journey, total) = tx.get_journey(c1.id, Page::default()).await.unwrap();
    assert_eq!((journey.len(), total), (3, 3));
    assert!(journey.iter().all(|i| i.customer_id == c1.id));
    assert!(journey[0].created_at >= journey[2].created_at);
    let (paged, total) = tx.get_journey(c1.id, page(1, 1)).await.unwrap();
    assert_eq!((paged.len(), total), (1, 3));

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
    assert_eq!((listed.len(), total), (2, 5));
    assert_eq!(tx.get_segment(both.id).await.unwrap().id, both.id);
    tx.delete_segment(both.id).await.unwrap();
    assert!(matches!(
        tx.get_segment(both.id).await,
        Err(Error::NotFound { kind: "segment" })
    ));
    assert!(matches!(
        tx.delete_segment(both.id).await,
        Err(Error::NotFound { kind: "segment" })
    ));
    assert!(matches!(
        tx.get_segment_members(both.id, Page::default()).await,
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
        make("s", json!({"created_after": "yesterday"}), None),
    ] {
        let e = tx.create_segment(&bad).await.unwrap_err();
        assert_eq!(e.code(), "invalid_argument", "{bad:?}");
    }
    assert_eq!(
        parse_json("criteria", "not json").unwrap_err().code(),
        "invalid_argument"
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn static_segments_hold_members(pool: PgPool) {
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

    tx.add_segment_member(seg.id, c1.id).await.unwrap();
    tx.add_segment_member(seg.id, c1.id).await.unwrap(); // idempotent
    tx.add_segment_member(seg.id, c2.id).await.unwrap();
    let (members, total) = tx
        .get_segment_members(seg.id, Page::default())
        .await
        .unwrap();
    assert_eq!((members.len(), total), (2, 2));

    tx.remove_segment_member(seg.id, c1.id).await.unwrap();
    assert!(matches!(
        tx.remove_segment_member(seg.id, c1.id).await,
        Err(Error::NotFound {
            kind: "segment member"
        })
    ));
    let (members, _) = tx
        .get_segment_members(seg.id, Page::default())
        .await
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, c2.id);

    // Deleting a customer drops their membership; deleting the segment keeps customers.
    tx.delete_customer(c2.id).await.unwrap();
    assert_eq!(
        tx.get_segment_members(seg.id, Page::default())
            .await
            .unwrap()
            .1,
        0
    );
    tx.delete_segment(seg.id).await.unwrap();
    tx.get_customer(c1.id).await.unwrap();

    let dynamic = tx
        .create_segment(&NewSegment {
            name: "d".into(),
            description: None,
            criteria: json!({}),
            segment_type: None,
        })
        .await
        .unwrap();
    let e = tx.add_segment_member(dynamic.id, c1.id).await.unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    assert!(matches!(
        tx.add_segment_member(Uuid::new_v4(), c1.id).await,
        Err(Error::NotFound { kind: "segment" })
    ));
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
        tx.record_interaction(&NewInteraction {
            customer_id: c.id,
            interaction_type: "login".into(),
            metadata: Some(json!({"n": i})),
        })
        .await
        .unwrap();
    }
    for (cust, subject, status) in [
        (c.id, "Broken", "in_progress"),
        (c.id, "Slow", "open"),
        (other.id, "Not mine", "open"),
    ] {
        sqlx::query(
            "INSERT INTO tickets (org_id, customer_id, subject, status, priority, channel) \
             VALUES ($1, $2, $3, $4::ticket_status, 'high', 'chat')",
        )
        .bind(org)
        .bind(cust)
        .bind(subject)
        .bind(status)
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
    assert_eq!(view.recent_interactions.len(), 10);
    assert_eq!(view.recent_tickets.len(), 2);
    assert!(view.recent_tickets.iter().all(|t| t.subject != "Not mine"));
    let broken = view
        .recent_tickets
        .iter()
        .find(|t| t.subject == "Broken")
        .unwrap();
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
    tx.add_segment_member(seg.id, cust.id).await.unwrap();
    tx.commit().await.unwrap();

    // Org B sees none of it.
    let mut tx = db.begin(b).await.unwrap();
    let all = CustomerFilter::default();
    let (rows, total) = tx.list_customers(&all, Page::default()).await.unwrap();
    assert!(rows.is_empty() && total == 0);
    assert!(tx
        .search_customers("alice", Page::default())
        .await
        .unwrap()
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
        assert!(matches!(
            tx.delete_segment(id).await,
            Err(Error::NotFound { .. })
        ));
    }
    assert!(matches!(
        tx.remove_segment_member(seg.id, cust.id).await,
        Err(Error::NotFound { .. })
    ));

    // B's own segment cannot take A's customer, and B's own customer count of
    // an all-matching dynamic segment excludes A's.
    let b_seg = tx
        .create_segment(&NewSegment {
            name: "B static".into(),
            description: None,
            criteria: json!({}),
            segment_type: Some("static".into()),
        })
        .await
        .unwrap();
    assert!(matches!(
        tx.add_segment_member(b_seg.id, cust.id).await,
        Err(Error::NotFound { kind: "customer" })
    ));
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
