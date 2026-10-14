//! The support domain (tickets, knowledge base, chat) against a real
//! Postgres, as `otto_app` pinned to an org.

mod common;

use common::{db, org};
use cxp_core::chat::ChatExt;
use cxp_core::knowledge::{ArticleUpdate, KnowledgeExt, NewArticle};
use cxp_core::tickets::{NewTicket, TicketFilter, TicketUpdate, TicketsExt};
use cxp_core::{Error, Page};
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
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

fn new_ticket(customer_id: Uuid, subject: &str) -> NewTicket {
    NewTicket {
        customer_id,
        subject: subject.into(),
        ..Default::default()
    }
}

fn not_found<T: std::fmt::Debug>(r: Result<T, Error>, kind: &str) {
    match r {
        Err(Error::NotFound { kind: k }) => assert_eq!(k, kind),
        other => panic!("expected NotFound({kind}), got {other:?}"),
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn ticket_lifecycle(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let cust = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();

    let t = tx
        .create_ticket(&NewTicket {
            description: Some("It broke".into()),
            channel: Some("web".into()),
            ..new_ticket(cust, "Card declined")
        })
        .await
        .unwrap();
    assert_eq!((t.status.as_str(), t.priority.as_str()), ("open", "medium"));
    assert_eq!(t.channel.as_deref(), Some("web"));
    assert!(t.resolved_at.is_none());
    assert_eq!(tx.get_ticket(t.id).await.unwrap().subject, "Card declined");

    let u = tx
        .update_ticket(
            t.id,
            &TicketUpdate {
                status: Some("in_progress".into()),
                priority: Some("urgent".into()),
                assigned_to: Some("sam".into()),
                tags: Some(vec!["billing".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (u.status.as_str(), u.priority.as_str()),
        ("in_progress", "urgent")
    );
    assert_eq!(u.assigned_to.as_deref(), Some("sam"));
    assert_eq!(u.tags, ["billing"]);
    assert_eq!(u.subject, "Card declined", "unset fields are kept");
    assert!(u.updated_at >= t.updated_at && u.resolved_at.is_none());

    let renamed = tx
        .update_ticket(
            t.id,
            &TicketUpdate {
                subject: Some("Card declined twice".into()),
                description: Some("again".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(renamed.subject, "Card declined twice");
    assert_eq!(renamed.status, "in_progress");

    let closed = tx.close_ticket(t.id).await.unwrap();
    assert_eq!(closed.status, "closed");
    assert!(closed.resolved_at.is_some());

    tx.delete_ticket(t.id).await.unwrap();
    not_found(tx.get_ticket(t.id).await, "ticket");
    not_found(tx.delete_ticket(t.id).await, "ticket");
    not_found(tx.close_ticket(t.id).await, "ticket");
    not_found(
        tx.update_ticket(t.id, &TicketUpdate::default()).await,
        "ticket",
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn resolved_at_follows_status(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let cust = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    let status = |s: &str| TicketUpdate {
        status: Some(s.into()),
        ..Default::default()
    };
    let t = tx.create_ticket(&new_ticket(cust, "x")).await.unwrap();

    let resolved = tx.update_ticket(t.id, &status("resolved")).await.unwrap();
    let first = resolved.resolved_at.expect("resolve stamps resolved_at");

    // Closing after resolving, by either route, keeps the first time.
    let closed = tx.close_ticket(t.id).await.unwrap();
    assert_eq!(
        (closed.status.as_str(), closed.resolved_at),
        ("closed", Some(first))
    );
    let again = tx.update_ticket(t.id, &status("closed")).await.unwrap();
    assert_eq!(again.resolved_at, Some(first));
    // An update that leaves status alone keeps it too.
    let tagged = tx
        .update_ticket(
            t.id,
            &TicketUpdate {
                tags: Some(vec!["a".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(tagged.resolved_at, Some(first));

    // Reopening clears it; resolving again stamps a new time.
    for s in ["open", "in_progress", "waiting"] {
        tx.update_ticket(t.id, &status("resolved")).await.unwrap();
        let r = tx.update_ticket(t.id, &status(s)).await.unwrap();
        assert_eq!((r.status.as_str(), r.resolved_at), (s, None));
    }
    let again = tx.update_ticket(t.id, &status("resolved")).await.unwrap();
    assert!(again.resolved_at.unwrap() > first);

    // Closing a never-resolved ticket stamps it.
    let u = tx.create_ticket(&new_ticket(cust, "y")).await.unwrap();
    assert!(tx.close_ticket(u.id).await.unwrap().resolved_at.is_some());
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn bad_enum_values_are_invalid_not_internal(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let cust = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();
    let bad_priority = NewTicket {
        priority: Some("asap".into()),
        ..new_ticket(cust, "x")
    };
    let bad_channel = NewTicket {
        channel: Some("fax".into()),
        ..new_ticket(cust, "x")
    };
    for bad in [bad_priority, bad_channel] {
        let e = tx.create_ticket(&bad).await.unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{e:?}");
        assert_eq!(e.code(), "invalid_argument");
    }
    let t = tx.create_ticket(&new_ticket(cust, "ok")).await.unwrap();
    let e = tx
        .update_ticket(
            t.id,
            &TicketUpdate {
                status: Some("done".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("in_progress"), "{e}");
    assert!(
        !e.to_string().contains("done"),
        "caller text is not echoed: {e}"
    );
    let e = tx
        .list_tickets(
            &TicketFilter {
                status: Some("nope".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Error::Invalid(_)));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn ticket_for_unknown_customer_is_not_found(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    not_found(
        tx.create_ticket(&new_ticket(Uuid::new_v4(), "x")).await,
        "customer",
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn ticket_filters_paging_and_total(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let a = customer(&db, org).await;
    let b = customer(&db, org).await;
    let experience = Uuid::new_v4();
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query("INSERT INTO experiences (id, org_id, name) VALUES ($1, $2, 'Checkout')")
        .bind(experience)
        .bind(org)
        .execute(tx.conn())
        .await
        .unwrap();

    let mut ids = Vec::new();
    for i in 0..5 {
        let t = tx
            .create_ticket(&NewTicket {
                priority: Some(if i < 2 { "high" } else { "low" }.into()),
                ..new_ticket(if i == 4 { b } else { a }, &format!("t{i}"))
            })
            .await
            .unwrap();
        ids.push(t.id);
    }
    tx.close_ticket(ids[0]).await.unwrap();
    sqlx::query("UPDATE tickets SET experience_id = $2 WHERE id = $1")
        .bind(ids[3])
        .bind(experience)
        .execute(tx.conn())
        .await
        .unwrap();

    let (all, total) = tx
        .list_tickets(&TicketFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!((all.len(), total), (5, 5));
    assert!(all.windows(2).all(|w| w[0].created_at >= w[1].created_at));

    let (p, total) = tx
        .list_tickets(&TicketFilter::default(), Page::new(Some(2), Some(4)))
        .await
        .unwrap();
    assert_eq!((p.len(), total), (1, 5), "total ignores the page");
    let (p, total) = tx
        .list_tickets(&TicketFilter::default(), Page::new(Some(2), Some(10)))
        .await
        .unwrap();
    assert_eq!((p.len(), total), (0, 5));

    let (rows, total) = tx
        .list_tickets(
            &TicketFilter {
                status: Some("closed".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (1, 1));
    let (_, total) = tx
        .list_tickets(
            &TicketFilter {
                priority: Some("high".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!(total, 2);
    let (_, total) = tx
        .list_tickets(
            &TicketFilter {
                customer_id: Some(b),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!(total, 1);
    let (rows, total) = tx
        .list_tickets(
            &TicketFilter {
                experience_id: Some(experience),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (1, 1));
    let (_, total) = tx
        .list_tickets(
            &TicketFilter {
                status: Some("open".into()),
                priority: Some("high".into()),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!(total, 1);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn knowledge_crud_and_get_by_id(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let a = tx
        .create_article(&NewArticle {
            title: "Refunds".into(),
            content: "How refunds work".into(),
            category: Some("billing".into()),
            tags: vec!["money".into()],
        })
        .await
        .unwrap();
    assert!(!a.published);
    assert_eq!(a.tags, ["money"]);
    assert_eq!(tx.get_article(a.id).await.unwrap().title, "Refunds");

    let u = tx
        .update_article(
            a.id,
            &ArticleUpdate {
                published: Some(true),
                content: Some("Refunds take five days".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(u.published);
    assert_eq!(
        (u.title.as_str(), u.category.as_deref()),
        ("Refunds", Some("billing"))
    );

    let (_, total) = tx.list_articles(Some(true), Page::default()).await.unwrap();
    assert_eq!(total, 1);
    let (rows, total) = tx
        .list_articles(Some(false), Page::default())
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (0, 0));

    tx.delete_article(a.id).await.unwrap();
    not_found(tx.get_article(a.id).await, "article");
    not_found(tx.delete_article(a.id).await, "article");
    not_found(
        tx.update_article(a.id, &ArticleUpdate::default()).await,
        "article",
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn knowledge_search_is_full_text_published_only_and_ranked(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let mut ids = std::collections::HashMap::new();
    for (title, content, published) in [
        (
            "Password reset",
            "Reset your password from the login page",
            true,
        ),
        (
            "Account security",
            "Passwords should be long. Passwords expire.",
            true,
        ),
        ("Draft passwords", "passwords passwords passwords", false),
        ("Shipping", "Orders ship in two days", true),
        ("100% off", "A coupon article", true),
    ] {
        let a = tx
            .create_article(&NewArticle {
                title: title.into(),
                content: content.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        if published {
            tx.update_article(
                a.id,
                &ArticleUpdate {
                    published: Some(true),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        ids.insert(title, a.id);
    }

    let found = tx.search_articles("password").await.unwrap();
    let titles: Vec<_> = found.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert!(
        !titles.contains(&"Draft passwords"),
        "unpublished is hidden"
    );
    assert!(!titles.contains(&"Shipping"));

    // Stemming: "ships" matches "ship"; a title substring matches too.
    assert_eq!(tx.search_articles("ships").await.unwrap().len(), 1);
    assert_eq!(tx.search_articles("Account sec").await.unwrap().len(), 1);
    assert!(tx.search_articles("zebra").await.unwrap().is_empty());
    // LIKE wildcards are literal.
    let pct = tx.search_articles("%").await.unwrap();
    assert_eq!(pct.len(), 1);
    assert_eq!(pct[0].id, ids["100% off"]);
    assert!(tx.search_articles("_").await.unwrap().is_empty());
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn knowledge_search_ranks_best_match_first_and_validates_the_query(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    for (title, content) in [
        (
            "Billing",
            "A refund can be requested within a month of purchase",
        ),
        ("Refund policy", "refund refund refund"),
    ] {
        let a = tx
            .create_article(&NewArticle {
                title: title.into(),
                content: content.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        tx.update_article(
            a.id,
            &ArticleUpdate {
                published: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let found = tx.search_articles("refund").await.unwrap();
    let titles: Vec<_> = found.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["Refund policy", "Billing"]);

    for bad in ["", "   ".to_string().as_str(), &"x".repeat(201)] {
        let e = tx.search_articles(bad).await.unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{e:?}");
    }
    assert!(tx
        .search_articles(&"x".repeat(200))
        .await
        .unwrap()
        .is_empty());
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn knowledge_search_is_capped(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query(
        "INSERT INTO knowledge_articles (org_id, title, content, published) \
         SELECT $1, 'Guide ' || n, 'gadget manual', true FROM generate_series(1, 25) n",
    )
    .bind(org)
    .execute(tx.conn())
    .await
    .unwrap();
    assert_eq!(tx.search_articles("gadget").await.unwrap().len(), 20);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn chat_lifecycle(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let cust = customer(&db, org).await;
    let mut tx = db.begin(org).await.unwrap();

    let s = tx.start_chat_session(cust).await.unwrap();
    assert_eq!((s.status.as_str(), s.customer_id), ("active", cust));
    assert!(s.ended_at.is_none());

    let m1 = tx.send_chat_message(s.id, "customer", "hi").await.unwrap();
    let m2 = tx.send_chat_message(s.id, "agent", "hello").await.unwrap();
    tx.send_chat_message(s.id, "system", "note").await.unwrap();
    assert_eq!((m1.role.as_str(), m1.session_id), ("customer", s.id));

    let (msgs, total) = tx.get_chat_messages(s.id, Page::default()).await.unwrap();
    assert_eq!(total, 3);
    let order: Vec<_> = msgs.iter().map(|m| m.id).collect();
    assert_eq!(&order[..2], &[m1.id, m2.id], "oldest first");
    let (msgs, total) = tx
        .get_chat_messages(s.id, Page::new(Some(1), Some(2)))
        .await
        .unwrap();
    assert_eq!((msgs.len(), total, msgs[0].role.as_str()), (1, 3, "system"));

    let e = tx.send_chat_message(s.id, "bot", "x").await.unwrap_err();
    assert!(matches!(e, Error::Invalid(_)), "{e:?}");

    let ended = tx.end_chat_session(s.id).await.unwrap();
    assert_eq!(ended.status, "ended");
    let first_end = ended.ended_at.expect("ended_at set");
    let again = tx.end_chat_session(s.id).await.unwrap();
    assert_eq!(again.ended_at, Some(first_end));
    assert_eq!(tx.get_chat_session(s.id).await.unwrap().status, "ended");
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn chat_unknown_references_are_not_found(pool: PgPool) {
    let db = db(pool);
    let org = org();
    // A failed foreign key aborts its transaction, so each case gets its own.
    not_found(
        db.begin(org)
            .await
            .unwrap()
            .start_chat_session(Uuid::new_v4())
            .await,
        "customer",
    );
    not_found(
        db.begin(org)
            .await
            .unwrap()
            .get_chat_session(Uuid::new_v4())
            .await,
        "chat session",
    );
    not_found(
        db.begin(org)
            .await
            .unwrap()
            .end_chat_session(Uuid::new_v4())
            .await,
        "chat session",
    );
    not_found(
        db.begin(org)
            .await
            .unwrap()
            .get_chat_messages(Uuid::new_v4(), Page::default())
            .await,
        "chat session",
    );
    not_found(
        db.begin(org)
            .await
            .unwrap()
            .send_chat_message(Uuid::new_v4(), "agent", "x")
            .await,
        "chat session",
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn org_b_cannot_see_or_touch_org_a(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    let cust_a = customer(&db, a).await;
    let cust_b = customer(&db, b).await;

    let mut tx = db.begin(a).await.unwrap();
    let ticket = tx
        .create_ticket(&new_ticket(cust_a, "secret"))
        .await
        .unwrap();
    let article = tx
        .create_article(&NewArticle {
            title: "Internal".into(),
            content: "zebra policy".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    tx.update_article(
        article.id,
        &ArticleUpdate {
            published: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let session = tx.start_chat_session(cust_a).await.unwrap();
    let msg = tx
        .send_chat_message(session.id, "customer", "hi")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin(b).await.unwrap();
    // Tickets.
    let (rows, total) = tx
        .list_tickets(&TicketFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (0, 0));
    let (rows, total) = tx
        .list_tickets(
            &TicketFilter {
                customer_id: Some(cust_a),
                ..Default::default()
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (0, 0));
    not_found(tx.get_ticket(ticket.id).await, "ticket");
    not_found(
        tx.update_ticket(
            ticket.id,
            &TicketUpdate {
                subject: Some("hijack".into()),
                ..Default::default()
            },
        )
        .await,
        "ticket",
    );
    not_found(tx.close_ticket(ticket.id).await, "ticket");
    not_found(tx.delete_ticket(ticket.id).await, "ticket");
    // Articles.
    not_found(tx.get_article(article.id).await, "article");
    assert!(tx.search_articles("zebra").await.unwrap().is_empty());
    assert_eq!(tx.list_articles(None, Page::default()).await.unwrap().1, 0);
    not_found(
        tx.update_article(
            article.id,
            &ArticleUpdate {
                title: Some("hijack".into()),
                ..Default::default()
            },
        )
        .await,
        "article",
    );
    not_found(tx.delete_article(article.id).await, "article");
    // Chat.
    not_found(tx.get_chat_session(session.id).await, "chat session");
    not_found(
        tx.get_chat_messages(session.id, Page::default()).await,
        "chat session",
    );
    not_found(tx.end_chat_session(session.id).await, "chat session");
    tx.commit().await.unwrap();
    // The FK refusal aborts a transaction, so it gets one of its own.
    let mut tx = db.begin(b).await.unwrap();
    not_found(
        tx.send_chat_message(session.id, "agent", "x").await,
        "chat session",
    );
    drop(tx);

    // A ticket or chat session for another org's customer is refused.
    // Each in its own transaction, since a refusal aborts it.
    let mut tx = db.begin(b).await.unwrap();
    not_found(tx.create_ticket(&new_ticket(cust_a, "x")).await, "customer");
    drop(tx);
    let mut tx = db.begin(b).await.unwrap();
    not_found(tx.start_chat_session(cust_a).await, "customer");
    drop(tx);
    // Org B's own customer still works.
    let mut tx = db.begin(b).await.unwrap();
    tx.create_ticket(&new_ticket(cust_b, "mine")).await.unwrap();
    tx.commit().await.unwrap();

    // Org A's data is intact.
    let mut tx = db.begin(a).await.unwrap();
    assert_eq!(tx.get_ticket(ticket.id).await.unwrap().subject, "secret");
    assert_eq!(tx.get_article(article.id).await.unwrap().title, "Internal");
    assert_eq!(
        tx.get_chat_session(session.id).await.unwrap().status,
        "active"
    );
    let (msgs, _) = tx
        .get_chat_messages(session.id, Page::default())
        .await
        .unwrap();
    assert_eq!(msgs[0].id, msg.id);
}
