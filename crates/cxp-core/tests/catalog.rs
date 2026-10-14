//! The experience catalog and the dashboard against a real Postgres, as
//! `otto_app` pinned to an org (`Db::begin`), so row-level security applies.

mod common;

use common::{db, org, team_platform};
use cxp_core::catalog::*;
use cxp_core::dashboard::DashboardExt;
use cxp_core::{Error, Page};
use otto_tenant::ids::OrgId;
use otto_tenant::Db;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn exp(name: &str) -> NewExperience {
    NewExperience {
        name: name.into(),
        ..Default::default()
    }
}

fn app(parent: Option<Uuid>, name: &str) -> NewApplication {
    NewApplication {
        experience_id: parent,
        name: name.into(),
        ..Default::default()
    }
}

fn page(parent: Option<Uuid>, name: &str) -> NewPage {
    NewPage {
        application_id: parent,
        name: name.into(),
        ..Default::default()
    }
}

fn comp(parent: Option<Uuid>, name: &str) -> NewComponent {
    NewComponent {
        page_id: parent,
        name: name.into(),
        ..Default::default()
    }
}

fn not_found(e: Error, kind: &str) {
    match e {
        Error::NotFound { kind: k } => assert_eq!(k, kind),
        other => panic!("expected NotFound({kind}), got {other:?}"),
    }
}

/// Run one statement as `org` with `$1` = the org and `$2` = `arg`.
async fn run_sql(db: &Db, org: OrgId, arg: Uuid, sql: &str) {
    let mut tx = db.begin(org).await.unwrap();
    sqlx::query(sql)
        .bind(org)
        .bind(arg)
        .execute(tx.conn())
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn experience_crud(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let platform = team_platform().await;
    let verified = platform.team(o).await;
    let team = verified.id();
    let mut tx = db.begin(o).await.unwrap();
    let e = tx
        .create_experience(&NewExperience {
            name: "Checkout".into(),
            description: Some("the flow".into()),
            team: Some(verified),
            metadata: Some(json!({"k": 1})),
        })
        .await
        .unwrap();
    assert_eq!(e.name, "Checkout");
    assert_eq!(e.team_id, Some(team));
    assert_eq!(e.metadata, json!({"k": 1}));
    assert_eq!(tx.get_experience(e.id).await.unwrap().name, "Checkout");

    let defaulted = tx.create_experience(&exp("Plain")).await.unwrap();
    assert_eq!(defaulted.metadata, json!({}));
    assert_eq!(defaulted.team_id, None);

    let u = tx
        .update_experience(
            e.id,
            &ExperienceUpdate {
                name: Some("Checkout v2".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(u.name, "Checkout v2");
    assert_eq!(u.description.as_deref(), Some("the flow"));
    assert_eq!(u.metadata, json!({"k": 1}));
    assert_eq!(u.team_id, Some(team));
    assert!(u.updated_at > e.updated_at);

    tx.delete_experience(e.id).await.unwrap();
    not_found(tx.get_experience(e.id).await.unwrap_err(), "experience");
    not_found(tx.delete_experience(e.id).await.unwrap_err(), "experience");
    not_found(
        tx.update_experience(e.id, &ExperienceUpdate::default())
            .await
            .unwrap_err(),
        "experience",
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn application_page_component_crud_and_fields(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let e = tx.create_experience(&exp("E")).await.unwrap();

    let a = tx
        .create_application(&NewApplication {
            experience_id: Some(e.id),
            name: "Web".into(),
            platform: Some("web".into()),
            description: Some("d".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(a.experience_id, Some(e.id));
    assert_eq!(a.platform.as_deref(), Some("web"));
    let a2 = tx
        .update_application(
            a.id,
            &ApplicationUpdate {
                platform: Some("ios".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (a2.platform.as_deref(), a2.name.as_str()),
        (Some("ios"), "Web")
    );
    assert_eq!(a2.experience_id, Some(e.id));

    let p = tx
        .create_page(&NewPage {
            application_id: Some(a.id),
            name: "Cart".into(),
            path: Some("/cart".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(p.path.as_deref(), Some("/cart"));
    let p2 = tx
        .update_page(
            p.id,
            &PageUpdate {
                path: Some("/basket".into()),
                metadata: Some(json!({"x": true})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(p2.path.as_deref(), Some("/basket"));
    assert_eq!(p2.metadata, json!({"x": true}));

    let c = tx
        .create_component(&NewComponent {
            page_id: Some(p.id),
            name: "Pay".into(),
            component_type: Some("button".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(c.component_type.as_deref(), Some("button"));
    let c2 = tx
        .update_component(
            c.id,
            &ComponentUpdate {
                component_type: Some("link".into()),
                description: Some("pay now".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(c2.component_type.as_deref(), Some("link"));
    assert_eq!(c2.description.as_deref(), Some("pay now"));
    assert_eq!(c2.page_id, Some(p.id));

    assert_eq!(tx.get_application(a.id).await.unwrap().id, a.id);
    assert_eq!(tx.get_page(p.id).await.unwrap().id, p.id);
    assert_eq!(tx.get_component(c.id).await.unwrap().id, c.id);

    tx.delete_component(c.id).await.unwrap();
    tx.delete_page(p.id).await.unwrap();
    tx.delete_application(a.id).await.unwrap();
    not_found(tx.get_component(c.id).await.unwrap_err(), "component");
    not_found(tx.delete_page(p.id).await.unwrap_err(), "page");
    not_found(
        tx.delete_application(a.id).await.unwrap_err(),
        "application",
    );
    not_found(
        tx.update_page(p.id, &PageUpdate::default())
            .await
            .unwrap_err(),
        "page",
    );
    not_found(
        tx.update_component(c.id, &ComponentUpdate::default())
            .await
            .unwrap_err(),
        "component",
    );
    not_found(
        tx.update_application(a.id, &ApplicationUpdate::default())
            .await
            .unwrap_err(),
        "application",
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn lists_filter_page_and_count(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let e1 = tx.create_experience(&exp("E1")).await.unwrap();
    let e2 = tx.create_experience(&exp("E2")).await.unwrap();
    let mut apps = vec![];
    for i in 0..3 {
        apps.push(
            tx.create_application(&app(Some(e1.id), &format!("a{i}")))
                .await
                .unwrap(),
        );
    }
    tx.create_application(&app(Some(e2.id), "other"))
        .await
        .unwrap();
    tx.create_application(&app(None, "loose")).await.unwrap();

    // Unfiltered: everything, newest first.
    let (all, total) = tx
        .list_applications(ApplicationFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!((all.len(), total), (5, 5));
    assert_eq!(all[0].name, "loose");

    // Filtered by experience; total counts the filter, not the page.
    let f = ApplicationFilter {
        experience_id: Some(e1.id),
    };
    let (rows, total) = tx
        .list_applications(f, Page::new(Some(2), None))
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (2, 3));
    assert_eq!(rows[0].name, "a2");
    let (rows, total) = tx
        .list_applications(f, Page::new(Some(2), Some(2)))
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (1, 3));
    assert_eq!(rows[0].name, "a0");
    let (rows, total) = tx
        .list_applications(f, Page::new(Some(2), Some(10)))
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (0, 3));

    let (exps, total) = tx.list_experiences(Page::new(Some(1), None)).await.unwrap();
    assert_eq!((exps.len(), total), (1, 2));
    assert_eq!(exps[0].name, "E2");

    // Pages by application, components by page.
    let p = tx.create_page(&page(Some(apps[0].id), "p1")).await.unwrap();
    tx.create_page(&page(Some(apps[1].id), "p2")).await.unwrap();
    tx.create_page(&page(None, "p3")).await.unwrap();
    let (rows, total) = tx
        .list_pages(
            PageFilter {
                application_id: Some(apps[0].id),
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (1, 1));
    let (_, total) = tx
        .list_pages(PageFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!(total, 3);

    tx.create_component(&comp(Some(p.id), "c1")).await.unwrap();
    tx.create_component(&comp(Some(p.id), "c2")).await.unwrap();
    tx.create_component(&comp(None, "c3")).await.unwrap();
    let (rows, total) = tx
        .list_components(
            ComponentFilter {
                page_id: Some(p.id),
            },
            Page::default(),
        )
        .await
        .unwrap();
    assert_eq!((rows.len(), total), (2, 2));
    let (_, total) = tx
        .list_components(ComponentFilter::default(), Page::default())
        .await
        .unwrap();
    assert_eq!(total, 3);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn team_keep_clear_set_on_every_entity(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let platform = team_platform().await;
    let (t1, t2) = (platform.team(o).await, platform.team(o).await);
    let mut tx = db.begin(o).await.unwrap();

    let e = tx
        .create_experience(&NewExperience {
            team: Some(t1),
            ..exp("E")
        })
        .await
        .unwrap();
    let kept = tx
        .update_experience(e.id, &ExperienceUpdate::default())
        .await
        .unwrap();
    assert_eq!(kept.team_id, Some(t1.id()));
    let set = tx
        .update_experience(
            e.id,
            &ExperienceUpdate {
                team: TeamChange::Set(t2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(set.team_id, Some(t2.id()));
    let cleared = tx
        .update_experience(
            e.id,
            &ExperienceUpdate {
                team: TeamChange::Clear,
                name: Some("renamed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!((cleared.team_id, cleared.name.as_str()), (None, "renamed"));
    // Set again from null.
    let again = tx
        .update_experience(
            e.id,
            &ExperienceUpdate {
                team: TeamChange::Set(t1),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(again.team_id, Some(t1.id()));

    let a = tx
        .create_application(&NewApplication {
            team: Some(t1),
            ..app(None, "A")
        })
        .await
        .unwrap();
    let a = tx
        .update_application(
            a.id,
            &ApplicationUpdate {
                team: TeamChange::Clear,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(a.team_id, None);
    let a = tx
        .update_application(
            a.id,
            &ApplicationUpdate {
                team: TeamChange::Set(t2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(a.team_id, Some(t2.id()));
    let a = tx
        .update_application(a.id, &ApplicationUpdate::default())
        .await
        .unwrap();
    assert_eq!(a.team_id, Some(t2.id()));

    let p = tx
        .create_page(&NewPage {
            team: Some(t1),
            ..page(None, "P")
        })
        .await
        .unwrap();
    let p = tx
        .update_page(
            p.id,
            &PageUpdate {
                team: TeamChange::Clear,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(p.team_id, None);
    let p = tx
        .update_page(
            p.id,
            &PageUpdate {
                team: TeamChange::Set(t2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(p.team_id, Some(t2.id()));
    assert_eq!(
        tx.update_page(p.id, &PageUpdate::default())
            .await
            .unwrap()
            .team_id,
        Some(t2.id())
    );

    let c = tx
        .create_component(&NewComponent {
            team: Some(t1),
            ..comp(None, "C")
        })
        .await
        .unwrap();
    let c = tx
        .update_component(
            c.id,
            &ComponentUpdate {
                team: TeamChange::Clear,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(c.team_id, None);
    let c = tx
        .update_component(
            c.id,
            &ComponentUpdate {
                team: TeamChange::Set(t2),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(c.team_id, Some(t2.id()));
    assert_eq!(
        tx.update_component(c.id, &ComponentUpdate::default())
            .await
            .unwrap()
            .team_id,
        Some(t2.id())
    );
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn deleting_a_parent_deletes_its_children_and_nulls_other_references(pool: PgPool) {
    let db = db(pool);
    let o = org();
    let mut tx = db.begin(o).await.unwrap();
    let e = tx.create_experience(&exp("E")).await.unwrap();
    let a = tx.create_application(&app(Some(e.id), "A")).await.unwrap();
    let p = tx.create_page(&page(Some(a.id), "P")).await.unwrap();
    let c = tx.create_component(&comp(Some(p.id), "C")).await.unwrap();
    let keep = tx.create_application(&app(None, "keep")).await.unwrap();
    // A customer pointing at the experience keeps existing, reference nulled.
    let cust = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO customers (id, org_id, email, experience_id, component_id) \
         VALUES ($1, $2, 'a@example.com', $3, $4)",
    )
    .bind(cust)
    .bind(o)
    .bind(e.id)
    .bind(c.id)
    .execute(tx.conn())
    .await
    .unwrap();

    tx.delete_experience(e.id).await.unwrap();
    not_found(tx.get_application(a.id).await.unwrap_err(), "application");
    not_found(tx.get_page(p.id).await.unwrap_err(), "page");
    not_found(tx.get_component(c.id).await.unwrap_err(), "component");
    tx.get_application(keep.id).await.unwrap();
    let (exp_id, comp_id): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT experience_id, component_id FROM customers WHERE id = $1")
            .bind(cust)
            .fetch_one(tx.conn())
            .await
            .unwrap();
    assert_eq!((exp_id, comp_id), (None, None));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_missing_parent_is_not_found(pool: PgPool) {
    let db = db(pool);
    let ghost = Some(Uuid::new_v4());
    // Each failed insert needs its own transaction: a refused statement aborts it.
    for (i, kind) in ["experience", "application", "page"]
        .into_iter()
        .enumerate()
    {
        let mut tx = db.begin(org()).await.unwrap();
        let err = match i {
            0 => tx.create_application(&app(ghost, "x")).await.map(|_| ()),
            1 => tx.create_page(&page(ghost, "x")).await.map(|_| ()),
            _ => tx.create_component(&comp(ghost, "x")).await.map(|_| ()),
        }
        .unwrap_err();
        not_found(err, kind);
    }
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn dashboard_counts_and_ticket_status(pool: PgPool) {
    let db = db(pool);
    let o = org();

    // Empty org: zeros and no ratings.
    {
        let mut tx = db.begin(o).await.unwrap();
        let m = tx.dashboard_metrics().await.unwrap();
        assert_eq!(
            (
                m.total_customers,
                m.total_tickets,
                m.open_tickets,
                m.total_feedback
            ),
            (0, 0, 0, 0)
        );
        let s = tx.dashboard_summary().await.unwrap();
        assert_eq!((s.csat_score, s.average_rating), (None, None));
        assert_eq!(tx.ticket_counts_by_status().await.unwrap().total, 0);
    }

    {
        let mut tx = db.begin(o).await.unwrap();
        tx.create_experience(&exp("E")).await.unwrap();
        tx.create_experience(&exp("E2")).await.unwrap();
        tx.commit().await.unwrap();
    }
    let customer = Uuid::new_v4();
    run_sql(
        &db,
        o,
        customer,
        "INSERT INTO customers (id, org_id, email) VALUES ($2, $1, 'a@example.com')",
    )
    .await;
    for (status, n) in [
        ("open", 3),
        ("in_progress", 2),
        ("waiting", 1),
        ("resolved", 4),
        ("closed", 5),
    ] {
        for _ in 0..n {
            let mut tx = db.begin(o).await.unwrap();
            sqlx::query(
                "INSERT INTO tickets (org_id, customer_id, subject, status) \
                 VALUES ($1, $2, 's', $3::ticket_status)",
            )
            .bind(o)
            .bind(customer)
            .bind(status)
            .execute(tx.conn())
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }
    }
    // Ratings 5, 4, 2, 1, and one unrated row, which neither average nor CSAT counts.
    for rating in [Some(5), Some(4), Some(2), Some(1), None] {
        let mut tx = db.begin(o).await.unwrap();
        sqlx::query(
            "INSERT INTO feedback (org_id, customer_id, feedback_type, rating) \
             VALUES ($1, $2, 'praise'::feedback_type, $3)",
        )
        .bind(o)
        .bind(customer)
        .bind(rating)
        .execute(tx.conn())
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    run_sql(
        &db,
        o,
        customer,
        "INSERT INTO surveys (org_id, title, questions) VALUES ($1, 'S', '[]')",
    )
    .await;

    let mut tx = db.begin(o).await.unwrap();
    let m = tx.dashboard_metrics().await.unwrap();
    assert_eq!(m.total_customers, 1);
    assert_eq!(m.total_tickets, 15);
    assert_eq!(m.open_tickets, 3);
    assert_eq!(m.total_feedback, 5);
    assert_eq!(m.total_surveys, 1);
    assert_eq!(m.total_experiences, 2);

    let c = tx.ticket_counts_by_status().await.unwrap();
    assert_eq!(
        (
            c.total,
            c.open,
            c.in_progress,
            c.waiting,
            c.resolved,
            c.closed
        ),
        (15, 3, 2, 1, 4, 5)
    );

    let s = tx.dashboard_summary().await.unwrap();
    assert_eq!(
        (
            s.total_tickets,
            s.open_tickets,
            s.in_progress_tickets,
            s.waiting_tickets,
            s.resolved_tickets,
            s.closed_tickets
        ),
        (15, 3, 2, 1, 4, 5)
    );
    assert_eq!(s.csat_score, Some(50.0));
    assert_eq!(s.average_rating, Some(3.0));
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn orgs_cannot_see_or_touch_each_others_catalog_or_counts(pool: PgPool) {
    let db = db(pool);
    let (a, b) = (org(), org());

    let (ea, aa, pa, ca) = {
        let mut tx = db.begin(a).await.unwrap();
        let e = tx.create_experience(&exp("A-exp")).await.unwrap();
        let ap = tx
            .create_application(&app(Some(e.id), "A-app"))
            .await
            .unwrap();
        let p = tx.create_page(&page(Some(ap.id), "A-page")).await.unwrap();
        let c = tx
            .create_component(&comp(Some(p.id), "A-comp"))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        (e.id, ap.id, p.id, c.id)
    };
    let customer = Uuid::new_v4();
    run_sql(
        &db,
        a,
        customer,
        "INSERT INTO customers (id, org_id, email) VALUES ($2, $1, 'a@x.io')",
    )
    .await;
    run_sql(
        &db,
        a,
        customer,
        "INSERT INTO tickets (org_id, customer_id, subject) VALUES ($1, $2, 's')",
    )
    .await;

    // Org B sees none of it.
    let mut tx = db.begin(b).await.unwrap();
    not_found(tx.get_experience(ea).await.unwrap_err(), "experience");
    not_found(tx.get_application(aa).await.unwrap_err(), "application");
    not_found(tx.get_page(pa).await.unwrap_err(), "page");
    not_found(tx.get_component(ca).await.unwrap_err(), "component");
    let (rows, total) = tx.list_experiences(Page::default()).await.unwrap();
    assert!(rows.is_empty());
    assert_eq!(total, 0);
    assert_eq!(
        tx.list_applications(ApplicationFilter::default(), Page::default())
            .await
            .unwrap()
            .1,
        0
    );
    assert_eq!(
        tx.list_applications(
            ApplicationFilter {
                experience_id: Some(ea)
            },
            Page::default()
        )
        .await
        .unwrap()
        .1,
        0
    );

    let none = |t: i64| assert_eq!(t, 0);
    none(
        tx.list_pages(
            PageFilter {
                application_id: Some(aa),
            },
            Page::default(),
        )
        .await
        .unwrap()
        .1,
    );
    none(
        tx.list_pages(PageFilter::default(), Page::default())
            .await
            .unwrap()
            .1,
    );
    none(
        tx.list_components(ComponentFilter { page_id: Some(pa) }, Page::default())
            .await
            .unwrap()
            .1,
    );
    none(
        tx.list_components(ComponentFilter::default(), Page::default())
            .await
            .unwrap()
            .1,
    );

    // ... cannot update or delete it.
    not_found(
        tx.update_application(aa, &ApplicationUpdate::default())
            .await
            .unwrap_err(),
        "application",
    );
    not_found(tx.delete_application(aa).await.unwrap_err(), "application");
    not_found(
        tx.update_page(pa, &PageUpdate::default())
            .await
            .unwrap_err(),
        "page",
    );
    not_found(
        tx.update_experience(
            ea,
            &ExperienceUpdate {
                name: Some("hijacked".into()),
                team: TeamChange::Clear,
                ..Default::default()
            },
        )
        .await
        .unwrap_err(),
        "experience",
    );
    not_found(
        tx.update_component(ca, &ComponentUpdate::default())
            .await
            .unwrap_err(),
        "component",
    );
    not_found(tx.delete_page(pa).await.unwrap_err(), "page");
    not_found(tx.delete_experience(ea).await.unwrap_err(), "experience");

    // Its dashboard excludes org A.
    let m = tx.dashboard_metrics().await.unwrap();
    assert_eq!(
        (
            m.total_customers,
            m.total_tickets,
            m.open_tickets,
            m.total_experiences
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(tx.ticket_counts_by_status().await.unwrap().total, 0);
    assert_eq!(tx.dashboard_summary().await.unwrap().total_tickets, 0);
    drop(tx);

    // A child under another org's parent is refused (composite FK).
    let mut tx = db.begin(b).await.unwrap();
    not_found(
        tx.create_application(&app(Some(ea), "x"))
            .await
            .unwrap_err(),
        "experience",
    );
    drop(tx);
    let mut tx = db.begin(b).await.unwrap();
    not_found(
        tx.create_page(&page(Some(aa), "x")).await.unwrap_err(),
        "application",
    );
    drop(tx);
    let mut tx = db.begin(b).await.unwrap();
    not_found(
        tx.create_component(&comp(Some(pa), "x")).await.unwrap_err(),
        "page",
    );
    drop(tx);

    // Org A's rows are untouched.
    let mut tx = db.begin(a).await.unwrap();
    assert_eq!(tx.get_experience(ea).await.unwrap().name, "A-exp");
    assert_eq!(tx.get_component(ca).await.unwrap().name, "A-comp");
    assert_eq!(tx.dashboard_metrics().await.unwrap().total_tickets, 1);
}

#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn blank_names_are_invalid_on_create_and_update(pool: PgPool) {
    let db = db(pool);
    let mut tx = db.begin(org()).await.unwrap();
    let e = tx.create_experience(&exp("E")).await.unwrap();
    let a = tx.create_application(&app(None, "A")).await.unwrap();
    let p = tx.create_page(&page(None, "P")).await.unwrap();
    let c = tx.create_component(&comp(None, "C")).await.unwrap();
    for blank in ["", "  \t "] {
        let invalid = |r: Result<(), Error>| assert!(matches!(r, Err(Error::Invalid(_))), "{r:?}");
        invalid(tx.create_experience(&exp(blank)).await.map(|_| ()));
        invalid(tx.create_application(&app(None, blank)).await.map(|_| ()));
        invalid(tx.create_page(&page(None, blank)).await.map(|_| ()));
        invalid(tx.create_component(&comp(None, blank)).await.map(|_| ()));
        let name = Some(blank.to_string());
        invalid(
            tx.update_experience(
                e.id,
                &ExperienceUpdate {
                    name: name.clone(),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ()),
        );
        invalid(
            tx.update_application(
                a.id,
                &ApplicationUpdate {
                    name: name.clone(),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ()),
        );
        invalid(
            tx.update_page(
                p.id,
                &PageUpdate {
                    name: name.clone(),
                    ..Default::default()
                },
            )
            .await
            .map(|_| ()),
        );
        invalid(
            tx.update_component(
                c.id,
                &ComponentUpdate {
                    name,
                    ..Default::default()
                },
            )
            .await
            .map(|_| ()),
        );
    }
    // Nothing was written, and the names are unchanged.
    assert_eq!(tx.list_experiences(Page::default()).await.unwrap().1, 1);
    assert_eq!(tx.get_experience(e.id).await.unwrap().name, "E");
}

/// A team id becomes storable only through `verify_team`: one the platform
/// knows in this org verifies; an unknown one, or another org's, is
/// `unknown_team`; a platform that cannot answer is `unavailable`.
#[tokio::test]
async fn verify_team_accepts_only_this_orgs_teams() {
    let platform = team_platform().await;
    let (o, other) = (org(), org());
    let mine = platform.add(o);
    let theirs = platform.add(other);

    let v = verify_team(&platform.client, o, mine).await.unwrap();
    assert_eq!(v.id(), mine);

    for id in [theirs, Uuid::new_v4()] {
        let e = verify_team(&platform.client, o, id).await.unwrap_err();
        assert!(matches!(e, Error::UnknownTeam), "{e:?}");
        assert_eq!(e.code(), "unknown_team");
    }

    platform.set_down(true);
    let e = verify_team(&platform.client, o, mine).await.unwrap_err();
    assert!(matches!(e, Error::Platform(_)), "{e:?}");
    assert_eq!(e.code(), "unavailable");
    assert!(e.retriable());
}

/// A team verified for one org is refused in another org's transaction, on
/// create and on update: the type carries the org it was checked for.
#[sqlx::test(migrator = "cxp_core::MIGRATOR")]
async fn a_team_verified_for_one_org_cannot_be_stored_in_another(pool: PgPool) {
    let db = db(pool);
    let platform = team_platform().await;
    let (a, b) = (org(), org());
    let a_team = platform.team(a).await;

    let mut tx = db.begin(b).await.unwrap();
    let e = tx
        .create_experience(&NewExperience {
            name: "B's".into(),
            team: Some(a_team),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(e, Error::UnknownTeam), "{e:?}");
    drop(tx);

    let mut tx = db.begin(b).await.unwrap();
    let own = tx.create_experience(&exp("B's own")).await.unwrap();
    let e = tx
        .update_experience(
            own.id,
            &ExperienceUpdate {
                team: TeamChange::Set(a_team),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(e, Error::UnknownTeam), "{e:?}");
}
