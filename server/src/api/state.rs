use sqlx::PgPool;

use crate::db;

#[derive(Clone)]
pub struct ApiState {
    pub customers: db::CustomerDb,
    pub tickets: db::TicketDb,
    pub feedback: db::FeedbackDb,
    pub knowledge: db::KnowledgeDb,
    pub interactions: db::InteractionDb,
    pub segments: db::SegmentDb,
    pub experiences: db::ExperienceDb,
    pub applications: db::ApplicationDb,
    pub pages: db::PageDb,
    pub components: db::ComponentDb,
}

impl ApiState {
    pub fn new(pool: PgPool) -> Self {
        Self {
            customers: db::CustomerDb::new(pool.clone()),
            tickets: db::TicketDb::new(pool.clone()),
            feedback: db::FeedbackDb::new(pool.clone()),
            knowledge: db::KnowledgeDb::new(pool.clone()),
            interactions: db::InteractionDb::new(pool.clone()),
            segments: db::SegmentDb::new(pool.clone()),
            experiences: db::ExperienceDb::new(pool.clone()),
            applications: db::ApplicationDb::new(pool.clone()),
            pages: db::PageDb::new(pool.clone()),
            components: db::ComponentDb::new(pool),
        }
    }
}
