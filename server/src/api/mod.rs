use axum::routing::get;
use axum::Router;
use tower_http::cors::{Any, CorsLayer};

pub mod applications;
pub mod components;
pub mod customers;
pub mod dashboard;
pub mod error;
pub mod experiences;
pub mod feedback;
pub mod pages;
pub mod pagination;
pub mod state;
pub mod surveys;
pub mod tickets;

pub use state::ApiState;

pub fn router(state: ApiState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
        // Dashboard
        .route("/dashboard/metrics", get(dashboard::metrics))
        // Customers
        .route("/customers", get(customers::list).post(customers::create))
        .route("/customers/search", get(customers::search))
        .route(
            "/customers/{id}",
            get(customers::get)
                .put(customers::update)
                .delete(customers::delete),
        )
        .route("/customers/{id}/360", get(customers::get_360))
        // Tickets
        .route("/tickets", get(tickets::list).post(tickets::create))
        .route(
            "/tickets/{id}",
            get(tickets::get)
                .put(tickets::update)
                .delete(tickets::delete),
        )
        // Feedback
        .route("/feedback", get(feedback::list))
        // Surveys
        .route("/surveys", get(surveys::list).post(surveys::create))
        .route(
            "/surveys/{id}",
            get(surveys::get)
                .put(surveys::update)
                .delete(surveys::delete),
        )
        .route("/surveys/{id}/responses", get(surveys::get_responses))
        // Experiences
        .route(
            "/experiences",
            get(experiences::list).post(experiences::create),
        )
        .route(
            "/experiences/{id}",
            get(experiences::get)
                .put(experiences::update)
                .delete(experiences::delete),
        )
        // Applications
        .route(
            "/applications",
            get(applications::list).post(applications::create),
        )
        .route(
            "/applications/{id}",
            get(applications::get)
                .put(applications::update)
                .delete(applications::delete),
        )
        // Pages
        .route("/pages", get(pages::list).post(pages::create))
        .route(
            "/pages/{id}",
            get(pages::get)
                .put(pages::update)
                .delete(pages::delete),
        )
        // Components
        .route(
            "/components",
            get(components::list).post(components::create),
        )
        .route(
            "/components/{id}",
            get(components::get)
                .put(components::update)
                .delete(components::delete),
        );

    Router::new()
        .nest("/api/v1", api)
        .with_state(state)
        .layer(cors)
}
