//! The support domain (tickets, knowledge base, chat) against a real
//! Postgres, as `cxp_app` pinned to an org.

mod common;

use common::{articles_both_ways, db, org};
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

    let found = tx.search_articles("password").await.unwrap().items;
    let titles: Vec<_> = found.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles.len(), 2, "{titles:?}");
    assert!(
        !titles.contains(&"Draft passwords"),
        "unpublished is hidden"
    );
    assert!(!titles.contains(&"Shipping"));

    // Stemming: "ships" matches "ship"; a title substring matches too.
    assert_eq!(tx.search_articles("ships").await.unwrap().items.len(), 1);
    assert_eq!(
        tx.search_articles("Account sec").await.unwrap().items.len(),
        1
    );
    assert!(tx.search_articles("zebra").await.unwrap().items.is_empty());
    // LIKE wildcards are literal.
    let pct = tx.search_articles("%").await.unwrap().items;
    assert_eq!(pct.len(), 1);
    assert_eq!(pct[0].id, ids["100% off"]);
    assert!(tx.search_articles("_").await.unwrap().items.is_empty());
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
    let found = tx.search_articles("refund").await.unwrap().items;
    let titles: Vec<_> = found.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["Refund policy", "Billing"]);

    let longest = cxp_core::search::MAX_QUERY_CHARS;
    for bad in ["", "   ".to_string().as_str(), &"x".repeat(longest + 1)] {
        let e = tx.search_articles(bad).await.unwrap_err();
        assert!(matches!(e, Error::Invalid(_)), "{e:?}");
    }
    assert!(tx
        .search_articles(&"x".repeat(longest))
        .await
        .unwrap()
        .items
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
    assert_eq!(tx.search_articles("gadget").await.unwrap().items.len(), 20);
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
    assert!(tx.search_articles("zebra").await.unwrap().items.is_empty());
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

// ------------------------------------------------- search under RLS (#17)

async fn seed_kb_corpus(tx: &mut otto_tenant::Tx<'_>, published_too: bool) {
    let articles: &[(&str, &str, bool)] = &[
        (
            "Reset your password",
            "Open settings, then security, and choose reset password.",
            true,
        ),
        (
            "Password policy",
            "Passwords need twelve characters. Resetting is free.",
            true,
        ),
        ("Draft: passwords", "passwords passwords passwords", false),
        (
            "Refund policy",
            "A refund can be requested within a month of purchase.",
            true,
        ),
        (
            "Shipping times",
            "Orders ship in two days; express shipping ships overnight.",
            true,
        ),
        (
            "100% off coupons",
            "How the coupon_code field works with 50% discounts.",
            true,
        ),
        (
            "E-mail setup",
            "Configure e-mail forwarding and the e-mail signature.",
            true,
        ),
        (
            "Über die Rückgabe",
            "Rückgabe innerhalb von 30 Tagen. Straße und Hausnummer.",
            true,
        ),
        ("Café menu", "The café serves crème brûlée.", true),
        (
            "API v2 webhooks",
            "POST https://api.example.com/v2/hooks with JSON; see RFC-7231.",
            true,
        ),
        ("The and of or", "the a an and of or to in", true),
        (
            "Account security",
            "Two-factor authentication keeps accounts safe.",
            true,
        ),
        ("中文 帮助", "如何 重置 密码", true),
        // "set" (four articles) is commoner than "password" (three), so a
        // query for both reads "password"'s articles first; "Reset
        // password" has that lexeme, lacks "set", and matches by title.
        (
            "Reset password",
            "Choose a new one on the sign-in page.",
            true,
        ),
        ("Set up a team", "Set the team name, then set roles.", true),
        ("Data sets", "Each set holds rows.", true),
        ("Settings overview", "All settings in one place.", true),
        // A title-only match for "reset account passw" that holds two of
        // its lexemes often, so ts_rank ranks it above both full matches.
        (
            "Reset account password",
            "Reset the account, then reset the account again.",
            true,
        ),
        ("Passw field", "The passw field resets accounts.", true),
        (
            "Old passw note",
            "It covers the account and much later a reset.",
            true,
        ),
    ];
    for (title, content, published) in articles {
        let a = tx
            .create_article(&NewArticle {
                title: (*title).into(),
                content: (*content).into(),
                ..Default::default()
            })
            .await
            .unwrap();
        if *published && published_too {
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
    }
}

/// Full-text matches (stemming, several words, stop words, hyphenated and
/// URL-like tokens, non-ASCII), title substrings of every length (one and
/// two characters take the prefix path), LIKE wildcards, and misses.
const KB_QUERIES: &[&str] = &[
    "password",
    "passwords",
    "reset password",
    "password reset",
    "resetting",
    "PASSWORD",
    "refund",
    "refunds policy",
    "ship",
    "ships",
    "shipping overnight",
    "express",
    "the",
    "the and",
    "of or",
    "e-mail",
    "email",
    "e-mail signature",
    "mail",
    "über",
    "Über",
    "rückgabe",
    "strasse",
    "straße",
    "café",
    "cafe",
    "crème brûlée",
    "https://api.example.com/v2/hooks",
    "api.example.com",
    "v2",
    "rfc-7231",
    "7231",
    "json",
    "100%",
    "%",
    "_",
    "coupon_code",
    "50%",
    "Account sec",
    "acc",
    "ac",
    "a",
    "r",
    "Re",
    "two-factor",
    "factor",
    "中文",
    "密码",
    "帮",
    "zebra",
    "x",
    "policy refund",
    "security account",
    "set password",
    "set",
    "settings",
    "reset account passw",
    "account passw",
    "passw",
];

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn indexed_article_search_returns_exactly_what_the_scan_returns(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());
    for o in [a, b] {
        let mut tx = db.begin(o).await.unwrap();
        seed_kb_corpus(&mut tx, true).await;
        tx.commit().await.unwrap();
    }
    let mut tx = db.begin(a).await.unwrap();
    // Twice over, so most comparisons run on the generic plan.
    for _ in 0..2 {
        for q in KB_QUERIES {
            if let Some((indexed, scanned)) = articles_both_ways(&mut tx, q).await {
                assert_eq!(indexed, scanned, "query {q:?}");
            }
        }
    }
    let mine: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM knowledge_articles WHERE org_id = $1")
        .bind(a)
        .fetch_all(tx.conn())
        .await
        .unwrap();
    let found = tx.search_articles("password").await.unwrap().items;
    assert!(!found.is_empty());
    assert!(
        found.iter().all(|x| mine.contains(&x.id)),
        "org B's articles leaked"
    );
}

/// Editing an article's title or content re-indexes it; publishing does not
/// need to; deleting it removes its terms.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn the_search_index_follows_article_writes(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let art = tx
        .create_article(&NewArticle {
            title: "Gift cards".into(),
            content: "Gift cards never expire".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    // Unpublished: indexed, but search still hides it.
    assert!(tx.search_articles("gift").await.unwrap().items.is_empty());
    tx.update_article(
        art.id,
        &ArticleUpdate {
            published: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(tx.search_articles("expire").await.unwrap().items.len(), 1);

    tx.update_article(
        art.id,
        &ArticleUpdate {
            title: Some("Store credit".into()),
            content: Some("Store credit lasts a year".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(tx.search_articles("expire").await.unwrap().items.is_empty());
    assert!(tx.search_articles("Gift c").await.unwrap().items.is_empty());
    assert_eq!(tx.search_articles("lasts").await.unwrap().items.len(), 1);
    assert_eq!(tx.search_articles("re cr").await.unwrap().items.len(), 1);

    tx.delete_article(art.id).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_search_terms")
        .fetch_one(tx.conn())
        .await
        .unwrap();
    assert_eq!(left, 0, "deleting the article leaves terms behind");
}

/// Past the cap, article search counts to it and says so. For a one-word
/// query whose matches all fit the read bound, the articles are still
/// exactly the scan's (they are chosen by the word's count of positions,
/// which orders `ts_rank` for one word); a several-word query's are matches
/// in `ts_rank` order. Below the cap, among the same articles, it is exact.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn article_search_counts_up_to_the_cap(pool: PgPool) {
    let db = db(pool);
    let org = org();
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query(
        "INSERT INTO knowledge_articles (org_id, title, content, published, created_at) \
         SELECT $1, 'Guide ' || n, repeat('gadget ', 1 + n % 40) || 'widget manual', true, \
                now() - make_interval(secs => n) \
         FROM generate_series(1, 1100) AS n",
    )
    .bind(org)
    .execute(tx.conn())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO knowledge_articles (org_id, title, content, published) \
         VALUES ($1, 'Rare gadget', 'the only sprocket', true)",
    )
    .bind(org)
    .execute(tx.conn())
    .await
    .unwrap();

    let (indexed, scanned) = articles_both_ways(&mut tx, "gadget").await.unwrap();
    assert_eq!((indexed.total, indexed.capped), (1000, true));
    assert_eq!(indexed.ids, scanned.ids, "one word: the scan's best");

    let found = tx.search_articles("gadget widget").await.unwrap();
    assert_eq!((found.total, found.total_capped), (1000, true));
    let ids: Vec<Uuid> = found.items.iter().map(|a| a.id).collect();
    assert_eq!(ids.len(), 20);
    let ordered: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM knowledge_articles WHERE org_id = $1 AND id = ANY ($2) \
           AND search_tsv @@ plainto_tsquery('english', 'gadget widget') \
         ORDER BY ts_rank(search_tsv, plainto_tsquery('english', 'gadget widget')) DESC, \
                  created_at DESC, id",
    )
    .bind(org)
    .bind(&ids)
    .fetch_all(tx.conn())
    .await
    .unwrap();
    assert_eq!(ordered, ids, "matches, in ts_rank order");

    for q in ["sprocket", "rare", "Guide 1099", "gadget sprocket"] {
        let (indexed, scanned) = articles_both_ways(&mut tx, q).await.unwrap();
        assert!(!indexed.capped, "{q:?}");
        assert_eq!(indexed, scanned, "{q:?}");
    }
}

/// Random articles over a small vocabulary (so words, stems, and title
/// substrings collide often) and random queries (word runs, title pieces
/// that span words) against the scan: identical whenever uncapped.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn random_article_corpora_search_like_the_scan(pool: PgPool) {
    const VOCAB: [&str; 22] = [
        "reset",
        "set",
        "settings",
        "setup",
        "password",
        "passwords",
        "pass",
        "word",
        "account",
        "accounts",
        "login",
        "sign",
        "in",
        "the",
        "of",
        "refund",
        "ship",
        "shipping",
        "order",
        "team",
        "data",
        "passw",
    ];
    let db = db(pool);
    let org = org();
    let mut rng = common::Rng::new(0x1717);
    let mut tx = db.begin(org).await.unwrap();
    let words = |rng: &mut common::Rng, n: usize| -> String {
        (0..n)
            .map(|_| VOCAB[rng.below(VOCAB.len())])
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut titles = Vec::new();
    for n in 0..150 {
        let (nt, nc) = (1 + rng.below(4), 3 + rng.below(13));
        let title = words(&mut rng, nt);
        let content = words(&mut rng, nc);
        sqlx::query(
            "INSERT INTO knowledge_articles (org_id, title, content, published, created_at) \
             VALUES ($1, $2, $3, $4, now() - make_interval(hours => $5))",
        )
        .bind(org)
        .bind(&title)
        .bind(&content)
        .bind(rng.below(7) > 0)
        .bind(n % 23)
        .execute(tx.conn())
        .await
        .unwrap();
        titles.push(title);
    }
    // Queries search answered, and how many of those were uncapped (exact):
    // a corpus that capped most of them would prove little.
    let (mut run, mut uncapped) = (0, 0);
    for i in 0..250 {
        let (nw, t) = (1 + rng.below(3), titles[rng.below(titles.len())].clone());
        let q = match i % 3 {
            0 => words(&mut rng, nw),
            1 => rng.substring(&t, 12),
            _ => {
                let at = rng.below(t.len().max(1));
                t.chars().skip(at).take(4 + rng.below(12)).collect()
            }
        };
        let Some((found, scanned)) = articles_both_ways(&mut tx, &q).await else {
            continue;
        };
        run += 1;
        uncapped += usize::from(!found.capped);
        if found.capped {
            assert!(found.total <= scanned.total, "{q:?}");
        } else {
            assert_eq!(found, scanned, "query {q:?}");
        }
    }
    // Measured when this bound was set (#34): 250 of 250 uncapped.
    // The corpus is fixed-seed, so the bound cannot flake: at least 95%.
    assert!(uncapped * 20 >= run * 19, "{uncapped} of {run} uncapped");
}

/// An article search past its `statement_timeout` is `query_too_broad`, as
/// a customer search is (`a_search_past_its_timeout_is_query_too_broad`).
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn an_article_search_past_its_timeout_is_query_too_broad(pool: PgPool) {
    let su = pool.clone();
    let db = db(pool);
    let org = org();
    // Committed first: an uncommitted insert would hold a lock on the index
    // table, and the `LOCK TABLE` below would wait on it.
    let mut tx = db.begin(org).await.unwrap();
    let art = tx
        .create_article(&NewArticle {
            title: "Timeouts".into(),
            content: "Anything at all".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    tx.update_article(
        art.id,
        &ArticleUpdate {
            published: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut lock = su.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *lock)
        .await
        .unwrap();
    sqlx::query("LOCK TABLE knowledge_search_terms IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();

    let mut tx = db.begin(org).await.unwrap();
    let started = std::time::Instant::now();
    let e = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tx.search_articles("anything"),
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

/// A title must fit its limit, on create and on update.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn article_titles_are_length_limited(pool: PgPool) {
    use cxp_core::knowledge::MAX_TITLE_CHARS;
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let a = tx
        .create_article(&NewArticle {
            title: "t".repeat(MAX_TITLE_CHARS),
            content: "fits".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let e = tx
        .create_article(&NewArticle {
            title: "é".repeat(MAX_TITLE_CHARS + 1),
            content: "too long".into(),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let e = tx
        .update_article(
            a.id,
            &ArticleUpdate {
                title: Some("é".repeat(MAX_TITLE_CHARS + 1)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
}

/// Content must fit its limit, on create and on update, counted in
/// characters (not bytes); a refused update changes nothing.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn article_content_is_length_limited(pool: PgPool) {
    use cxp_core::knowledge::MAX_CONTENT_CHARS;
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let a = tx
        .create_article(&NewArticle {
            title: "At the limit".into(),
            content: "é".repeat(MAX_CONTENT_CHARS),
            ..Default::default()
        })
        .await
        .unwrap();
    let e = tx
        .create_article(&NewArticle {
            title: "Too long".into(),
            content: "é".repeat(MAX_CONTENT_CHARS + 1),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let e = tx
        .update_article(
            a.id,
            &ArticleUpdate {
                content: Some("é".repeat(MAX_CONTENT_CHARS + 1)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    let unchanged = tx.get_article(a.id).await.unwrap();
    assert_eq!(unchanged.content.chars().count(), MAX_CONTENT_CHARS);
}
