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

/// `?limit=&offset=` on its own.
#[derive(Debug, Default, Deserialize)]
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
            "dashboardMetrics": super::dashboard::Metrics {
                total_customers: 1,
                total_tickets: 2,
                open_tickets: 1,
                total_feedback: 3,
                total_surveys: 1,
                total_experiences: 1,
            },
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
