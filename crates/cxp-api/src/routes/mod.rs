//! The `/api/v1` handlers, one module per resource.
//! Casing rule: response models are camelCase (the `cxp-core` models' serde
//! shape), except `GET /dashboard/metrics`, which keeps its snake_case keys.
//! Request bodies and query parameters are snake_case; catalog bodies also
//! accept `teamId` as a permanent alias of `team_id`. The web console's types
//! follow this (#15); a single convention for `/api/v1` is tracked in #24.

pub mod catalog;
pub mod customers;
pub mod dashboard;
pub mod feedback;
pub mod me;
pub mod surveys;
pub mod tickets;

use serde::{Deserialize, Serialize};

/// A list response: one page of rows and the total matching the filter.
/// `limit` and `offset` are the page actually served (clamped by
/// [`cxp_core::Page`]: limit 1..=200, default 50; offset never negative).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Paginated<T: Serialize> {
    pub data: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl<T: Serialize> Paginated<T> {
    pub fn new((data, total): (Vec<T>, i64), page: cxp_core::Page) -> Self {
        Self {
            data,
            total,
            limit: page.limit(),
            offset: page.offset(),
        }
    }
}

/// A search response: [`Paginated`] plus `totalCapped`. `total` counts
/// matches up to [`cxp_core::search::TOTAL_CAP`]; `totalCapped: false` means
/// `total` and `data` are exact, `true` that `total` is a lower bound (more
/// matches than the cap, or the search stopped before counting them all),
/// `data` is then the requested page of the best-ranked matches found, and a
/// page starting at or past the cap is empty.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CappedPaginated<T: Serialize> {
    pub data: Vec<T>,
    pub total: i64,
    pub total_capped: bool,
    pub limit: i64,
    pub offset: i64,
}

impl<T: Serialize> CappedPaginated<T> {
    pub fn new(found: cxp_core::search::SearchPage<T>, page: cxp_core::Page) -> Self {
        Self {
            data: found.items,
            total: found.total,
            total_capped: found.total_capped,
            limit: page.limit(),
            offset: page.offset(),
        }
    }
}

/// `?limit=&offset=` on its own.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl PageParams {
    pub fn page(&self) -> cxp_core::Page {
        cxp_core::Page::new(self.limit, self.offset)
    }
}

/// The body every successful delete answers with.
pub fn deleted() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "deleted": true }))
}

/// The JSON the console reads, pinned to a fixture the web app's tests also
/// load (`apps/web/src/lib/types.test.ts`), so the two sides cannot drift
/// apart unnoticed (#15). Regenerate with `UPDATE_WIRE_FIXTURES=1 cargo test
/// -p cxp-api wire_fixture`.
#[cfg(test)]
mod wire_fixture {
    use chrono::{DateTime, TimeZone, Utc};
    use cxp_core::catalog::{Application, Component, Experience, Page};
    use cxp_core::customers::{Customer, Customer360, FeedbackSummary, TicketSummary};
    use cxp_core::feedback::{Feedback, Survey, SurveyResponse};
    use cxp_core::interactions::Interaction;
    use cxp_core::tickets::Ticket;
    use serde_json::json;
    use uuid::Uuid;

    use super::Paginated;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wire.json");

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, day, 12, 30, 0).unwrap()
    }

    fn customer() -> Customer {
        Customer {
            id: id(1),
            email: "ada@example.com".into(),
            name: Some("Ada Lovelace".into()),
            phone: Some("+15550100".into()),
            company: Some("Analytical Engines".into()),
            metadata: json!({ "plan": "pro" }),
            tags: vec!["vip".into()],
            experience_id: Some(id(10)),
            application_id: Some(id(11)),
            page_id: Some(id(12)),
            component_id: Some(id(13)),
            created_at: at(1),
            updated_at: at(2),
        }
    }

    fn ticket() -> Ticket {
        Ticket {
            id: id(2),
            customer_id: id(1),
            subject: "Cannot log in".into(),
            description: Some("The login button does nothing".into()),
            status: "in_progress".into(),
            priority: "high".into(),
            channel: Some("email".into()),
            assigned_to: Some("grace".into()),
            tags: vec!["login".into()],
            metadata: json!({}),
            experience_id: Some(id(10)),
            application_id: Some(id(11)),
            page_id: Some(id(12)),
            component_id: Some(id(13)),
            resolved_at: Some(at(4)),
            created_at: at(3),
            updated_at: at(4),
        }
    }

    fn feedback() -> Feedback {
        Feedback {
            id: id(3),
            customer_id: id(1),
            feedback_type: "nps".into(),
            subject: Some("Great".into()),
            body: Some("Loved it".into()),
            rating: Some(9),
            metadata: json!({}),
            experience_id: Some(id(10)),
            application_id: Some(id(11)),
            page_id: Some(id(12)),
            component_id: Some(id(13)),
            created_at: at(5),
        }
    }

    fn survey() -> Survey {
        Survey {
            id: id(4),
            title: "Quarterly NPS".into(),
            description: Some("How are we doing?".into()),
            questions: json!([{ "id": "q1", "text": "Score us", "type": "rating" }]),
            active: true,
            experience_id: Some(id(10)),
            application_id: Some(id(11)),
            page_id: Some(id(12)),
            component_id: Some(id(13)),
            created_at: at(6),
            updated_at: at(7),
        }
    }

    fn survey_response() -> SurveyResponse {
        SurveyResponse {
            id: id(5),
            survey_id: id(4),
            customer_id: id(1),
            answers: json!({ "q1": 9 }),
            completed_at: Some(at(8)),
            created_at: at(8),
        }
    }

    fn interaction() -> Interaction {
        Interaction {
            id: id(6),
            customer_id: id(1),
            interaction_type: "page_view".into(),
            metadata: json!({ "path": "/pricing" }),
            created_at: at(9),
        }
    }

    fn experience() -> Experience {
        Experience {
            id: id(10),
            name: "Storefront".into(),
            description: Some("The shop".into()),
            team_id: Some(id(20)),
            metadata: json!({}),
            created_at: at(10),
            updated_at: at(11),
        }
    }

    fn application() -> Application {
        Application {
            id: id(11),
            experience_id: Some(id(10)),
            name: "Web".into(),
            platform: Some("web".into()),
            description: Some("The web app".into()),
            team_id: Some(id(20)),
            metadata: json!({}),
            created_at: at(12),
            updated_at: at(13),
        }
    }

    fn page() -> Page {
        Page {
            id: id(12),
            application_id: Some(id(11)),
            name: "Checkout".into(),
            path: Some("/checkout".into()),
            description: Some("Pay here".into()),
            team_id: Some(id(20)),
            metadata: json!({}),
            created_at: at(14),
            updated_at: at(15),
        }
    }

    fn component() -> Component {
        Component {
            id: id(13),
            page_id: Some(id(12)),
            name: "Pay button".into(),
            component_type: Some("button".into()),
            description: Some("Submits payment".into()),
            team_id: Some(id(20)),
            metadata: json!({}),
            created_at: at(16),
            updated_at: at(17),
        }
    }

    fn customer_360() -> Customer360 {
        Customer360 {
            customer: customer(),
            recent_tickets: vec![TicketSummary {
                id: id(2),
                subject: "Cannot log in".into(),
                status: "in_progress".into(),
                priority: "high".into(),
                channel: Some("email".into()),
                assigned_to: Some("grace".into()),
                created_at: at(3),
                resolved_at: Some(at(4)),
            }],
            recent_interactions: vec![interaction()],
            recent_feedback: vec![FeedbackSummary {
                id: id(3),
                feedback_type: "nps".into(),
                subject: Some("Great".into()),
                body: Some("Loved it".into()),
                rating: Some(9),
                created_at: at(5),
            }],
        }
    }

    /// A customer with every optional field unset, so a `null` is pinned.
    fn customer_minimal() -> Customer {
        Customer {
            id: id(30),
            email: "min@example.com".into(),
            name: None,
            phone: None,
            company: None,
            metadata: json!({}),
            tags: vec![],
            experience_id: None,
            application_id: None,
            page_id: None,
            component_id: None,
            created_at: at(1),
            updated_at: at(1),
        }
    }

    /// An open ticket with every optional field unset.
    fn ticket_open() -> Ticket {
        Ticket {
            id: id(31),
            customer_id: id(30),
            subject: "Open".into(),
            description: None,
            status: "open".into(),
            priority: "medium".into(),
            channel: None,
            assigned_to: None,
            tags: vec![],
            metadata: json!({}),
            experience_id: None,
            application_id: None,
            page_id: None,
            component_id: None,
            resolved_at: None,
            created_at: at(3),
            updated_at: at(3),
        }
    }

    /// One sample of every response shape the console decodes.
    fn payloads() -> serde_json::Value {
        let page = cxp_core::Page::new(Some(25), Some(0));
        json!({
            "customer": customer(),
            "customerMinimal": customer_minimal(),
            "ticket": ticket(),
            "ticketOpen": ticket_open(),
            "feedback": feedback(),
            "survey": survey(),
            "surveyResponse": survey_response(),
            "interaction": interaction(),
            "experience": experience(),
            "application": application(),
            "page": self::page(),
            "component": component(),
            "customer360": customer_360(),
            "team": super::me::Team {
                id: id(20),
                slug: "growth".into(),
                name: "Growth".into(),
            },
            "dashboardMetrics": super::dashboard::Metrics::from(
                cxp_core::dashboard::DashboardMetrics {
                    total_customers: 1,
                    total_tickets: 2,
                    open_tickets: 1,
                    total_feedback: 3,
                    total_surveys: 1,
                    total_experiences: 1,
                }
            ),
            "customerSearch": super::CappedPaginated::new(
                cxp_core::search::SearchPage {
                    items: vec![customer()],
                    total: 1,
                    total_capped: false,
                },
                page
            ),
            "customerList": Paginated::new((vec![customer()], 1), page),
            "ticketList": Paginated::new((vec![ticket()], 1), page),
            "feedbackList": Paginated::new((vec![feedback()], 1), page),
            "surveyList": Paginated::new((vec![survey()], 1), page),
            "experienceList": Paginated::new((vec![experience()], 1), page),
            "applicationList": Paginated::new((vec![application()], 1), page),
            "pageList": Paginated::new((vec![self::page()], 1), page),
            "componentList": Paginated::new((vec![component()], 1), page),
        })
    }

    #[test]
    fn console_payloads_match_the_committed_fixture() {
        let actual = serde_json::to_string_pretty(&payloads()).unwrap() + "\n";
        if std::env::var_os("UPDATE_WIRE_FIXTURES").is_some() {
            std::fs::create_dir_all(std::path::Path::new(FIXTURE).parent().unwrap()).unwrap();
            std::fs::write(FIXTURE, &actual).unwrap();
            panic!("fixture regenerated; re-run without UPDATE_WIRE_FIXTURES");
        }
        let committed = std::fs::read_to_string(FIXTURE).unwrap_or_default();
        assert_eq!(
            committed, actual,
            "a response shape changed: regenerate {FIXTURE} with UPDATE_WIRE_FIXTURES=1 and update apps/web/src/lib/types.ts"
        );
    }
}

/// Request structs take camelCase keys and, as aliases, the snake_case
/// spellings `/api/v1` accepted before #24, so older callers keep working.
#[cfg(test)]
mod casing {
    use axum::extract::Query;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use uuid::Uuid;

    fn body<T: DeserializeOwned>(v: serde_json::Value) -> T {
        serde_json::from_value(v).unwrap()
    }

    fn query<T: DeserializeOwned>(qs: &str) -> T {
        let uri: http::Uri = format!("/x?{qs}").parse().unwrap();
        Query::<T>::try_from_uri(&uri).unwrap().0
    }

    fn query_err<T: DeserializeOwned>(qs: &str) -> bool {
        let uri: http::Uri = format!("/x?{qs}").parse().unwrap();
        Query::<T>::try_from_uri(&uri).is_err()
    }

    const A: Uuid = Uuid::from_u128(1);
    const B: Uuid = Uuid::from_u128(2);
    const C: Uuid = Uuid::from_u128(3);
    const D: Uuid = Uuid::from_u128(4);

    /// `experienceId=A&applicationId=B&pageId=C&componentId=D` in one spelling.
    fn catalog_qs(camel: bool) -> String {
        if camel {
            format!("experienceId={A}&applicationId={B}&pageId={C}&componentId={D}")
        } else {
            format!("experience_id={A}&application_id={B}&page_id={C}&component_id={D}")
        }
    }

    #[test]
    fn customer_list_params_take_both_spellings() {
        for camel in [true, false] {
            let q: super::customers::ListParams =
                query(&format!("limit=5&offset=1&{}", catalog_qs(camel)));
            assert_eq!((q.limit, q.offset), (Some(5), Some(1)));
            assert_eq!(q.experience_id, Some(A));
            assert_eq!(q.application_id, Some(B));
            assert_eq!(q.page_id, Some(C));
            assert_eq!(q.component_id, Some(D));
        }
    }

    #[test]
    fn survey_list_params_take_both_spellings() {
        for camel in [true, false] {
            let q: super::surveys::ListParams = query(&catalog_qs(camel));
            assert_eq!(
                (q.experience_id, q.application_id, q.page_id, q.component_id),
                (Some(A), Some(B), Some(C), Some(D))
            );
        }
    }

    #[test]
    fn ticket_params_and_bodies_take_both_spellings() {
        for (customer, rest) in [
            ("customerId", catalog_qs(true)),
            ("customer_id", catalog_qs(false)),
        ] {
            let q: super::tickets::ListParams =
                query(&format!("status=open&priority=high&{customer}={A}&{rest}"));
            assert_eq!(q.customer_id, Some(A));
            assert_eq!(q.experience_id, Some(A));
            assert_eq!(q.application_id, Some(B));
            assert_eq!(q.page_id, Some(C));
            assert_eq!(q.component_id, Some(D));
            assert_eq!(q.status.as_deref(), Some("open"));
        }
        for key in ["customerId", "customer_id"] {
            let b: super::tickets::CreateBody = body(json!({ key: A, "subject": "s" }));
            assert_eq!(b.customer_id, A);
        }
        for key in ["assignedTo", "assigned_to"] {
            let b: super::tickets::UpdateBody = body(json!({ key: "grace" }));
            assert_eq!(b.assigned_to.as_deref(), Some("grace"));
        }
    }

    #[test]
    fn feedback_params_take_both_spellings() {
        for (customer, kind, rest) in [
            ("customerId", "feedbackType", catalog_qs(true)),
            ("customer_id", "feedback_type", catalog_qs(false)),
        ] {
            let q: super::feedback::ListParams =
                query(&format!("{customer}={A}&{kind}=nps&{rest}"));
            assert_eq!(q.customer_id, Some(A));
            assert_eq!(q.feedback_type.as_deref(), Some("nps"));
            assert_eq!(
                (q.experience_id, q.application_id, q.page_id, q.component_id),
                (Some(A), Some(B), Some(C), Some(D))
            );
        }
    }

    #[test]
    fn catalog_params_and_bodies_take_both_spellings() {
        use super::catalog::*;
        for camel in [true, false] {
            let (exp, app, page, kind, team) = if camel {
                (
                    "experienceId",
                    "applicationId",
                    "pageId",
                    "componentType",
                    "teamId",
                )
            } else {
                (
                    "experience_id",
                    "application_id",
                    "page_id",
                    "component_type",
                    "team_id",
                )
            };
            let q: ApplicationParams = query(&format!("{exp}={A}"));
            assert_eq!(q.experience_id, Some(A));
            let q: PageParamsByApplication = query(&format!("{app}={B}"));
            assert_eq!(q.application_id, Some(B));
            let q: ComponentParams = query(&format!("{page}={C}"));
            assert_eq!(q.page_id, Some(C));

            let b: CreateExperienceBody = body(json!({ "name": "e", team: D }));
            assert_eq!(b.team_id, Some(D));
            let b: UpdateExperienceBody = body(json!({ team: D }));
            assert_eq!(b.team_id, Some(Some(D)));
            let b: CreateApplicationBody = body(json!({ "name": "a", exp: A, team: D }));
            assert_eq!((b.experience_id, b.team_id), (Some(A), Some(D)));
            let b: UpdateApplicationBody = body(json!({ team: null }));
            assert_eq!(b.team_id, Some(None));
            let b: CreatePageBody = body(json!({ "name": "p", app: B, team: D }));
            assert_eq!((b.application_id, b.team_id), (Some(B), Some(D)));
            let b: UpdatePageBody = body(json!({ team: D }));
            assert_eq!(b.team_id, Some(Some(D)));
            let b: CreateComponentBody =
                body(json!({ "name": "c", page: C, kind: "button", team: D }));
            assert_eq!((b.page_id, b.team_id), (Some(C), Some(D)));
            assert_eq!(b.component_type.as_deref(), Some("button"));
            let b: UpdateComponentBody = body(json!({ kind: "link", team: D }));
            assert_eq!(b.component_type.as_deref(), Some("link"));
            assert_eq!(b.team_id, Some(Some(D)));
        }
    }

    #[test]
    fn both_spellings_of_one_field_are_refused() {
        use super::catalog::*;
        /// The error serde gives for a key sent in both spellings.
        fn duplicate<T: DeserializeOwned + std::fmt::Debug>(v: serde_json::Value) {
            let err = serde_json::from_value::<T>(v).unwrap_err().to_string();
            assert!(err.contains("duplicate field"), "{err}");
        }
        duplicate::<super::tickets::CreateBody>(
            json!({ "customerId": A, "customer_id": B, "subject": "s" }),
        );
        duplicate::<super::tickets::UpdateBody>(json!({ "assignedTo": "a", "assigned_to": "b" }));
        duplicate::<UpdateComponentBody>(json!({ "componentType": "a", "component_type": "b" }));
        duplicate::<CreateExperienceBody>(json!({ "name": "e", "teamId": A, "team_id": B }));
        duplicate::<UpdateExperienceBody>(json!({ "teamId": A, "team_id": null }));
        assert!(query_err::<super::customers::ListParams>(&format!(
            "pageId={A}&page_id={B}"
        )));
    }

    /// Each field takes either spelling on its own, so an older caller can
    /// mix them while it migrates.
    #[test]
    fn spellings_mix_per_field() {
        let q: super::tickets::ListParams = query(&format!("customerId={A}&page_id={C}"));
        assert_eq!((q.customer_id, q.page_id), (Some(A), Some(C)));
    }

    /// An update body without the team key keeps the team (`None`), in
    /// either casing of the rest of the body.
    #[test]
    fn an_absent_team_key_keeps_the_team() {
        let b: super::catalog::UpdatePageBody = body(json!({ "name": "p" }));
        assert_eq!(b.team_id, None);
        let b: super::catalog::UpdateComponentBody = body(json!({ "componentType": "x" }));
        assert_eq!(b.team_id, None);
    }
}
