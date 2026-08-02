//! Axum router construction.

use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

use crate::handlers;
use crate::service::JobService;

/// The shared router state.
#[derive(Clone)]
pub struct RouterState {
    pub service: Arc<dyn JobService>,
}

impl std::fmt::Debug for RouterState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouterState").finish_non_exhaustive()
    }
}

/// The maximum accepted request body (input frames are small).
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// Build the complete application router.
pub fn build_router(service: Arc<dyn JobService>) -> Router {
    let state = RouterState { service };

    Router::new()
        .route("/", get(handlers::index))
        .route("/jobs", get(handlers::jobs_index))
        .route("/jobs/:id", get(handlers::job_page))
        .route("/login", get(handlers::login_page))
        .route("/static/app.js", get(handlers::app_js))
        .route("/static/app.css", get(handlers::app_css))
        // API.
        .route("/api/v1/server", get(handlers::server_info))
        .route("/api/v1/auth", get(handlers::auth_status))
        .route("/api/v1/login", post(handlers::login))
        .route("/api/v1/logout", post(handlers::logout))
        .route("/api/v1/jobs", get(handlers::list_jobs))
        .route(
            "/api/v1/jobs/:id",
            get(handlers::get_job).delete(handlers::delete_job),
        )
        .route("/api/v1/jobs/:id/output", get(handlers::get_output))
        .route("/api/v1/jobs/:id/events", get(handlers::get_events))
        .route(
            "/api/v1/jobs/:id/signals/:signal",
            post(handlers::send_signal),
        )
        .route("/api/v1/jobs/:id/input", post(handlers::send_input))
        .route("/api/v1/jobs/:id/ws", get(handlers::job_ws))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}
