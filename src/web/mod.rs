//! Web server for the Crucible dashboard.
//!
//! Provides a browser-based UI with:
//! - Elo timeline chart (the main view)
//! - Live game viewer via WebSocket
//! - Job queue management
//! - Bisect visualization
//!
//! The web UI is served as a single embedded HTML page
//! with all JS/CSS inline for zero-dependency deployment.

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use std::sync::Arc;

use crate::storage::Storage;

pub struct WebState {
    pub storage: Storage,
}

pub fn create_router(storage: Storage) -> Router {
    let state = Arc::new(WebState { storage });

    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/api/engines", get(engines_handler))
        .route("/api/timeline/{engine_id}", get(timeline_handler))
        .route("/api/jobs", get(jobs_handler))
        .route("/api/bisect", get(bisect_handler))
        .with_state(state)
}

async fn index_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn status_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_system_status() {
        Ok(status) => Json(serde_json::to_value(status).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn engines_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_engines() {
        Ok(engines) => Json(serde_json::to_value(engines).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn timeline_handler(
    State(state): State<Arc<WebState>>,
    axum::extract::Path(engine_id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.storage.get_elo_timeline(&engine_id, None) {
        Ok(timeline) => Json(serde_json::to_value(timeline).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn jobs_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.list_recent_jobs(50) {
        Ok(jobs) => Json(serde_json::json!({ "jobs": jobs })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn bisect_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_running_bisect_sessions() {
        Ok(sessions) => Json(serde_json::to_value(sessions).unwrap()).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// The entire dashboard as a single embedded HTML page.
/// Loaded from templates/ at compile time — zero-dependency deployment,
/// but you get proper syntax highlighting and editor support in the .html file.
const DASHBOARD_HTML: &str = include_str!("../../templates/dashboard.html");
