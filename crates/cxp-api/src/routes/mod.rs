//! The `/api/v1` handlers, one module per resource as before the port.
//! Request bodies and query parameters keep their snake_case names; response
//! models are camelCase (the `cxp-core` models' serde shape).

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
