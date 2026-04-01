//! Web server for the Crucible dashboard and admin surface.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::bisect::{BisectRunner, BisectStep};
use crate::config::Config;
use crate::git::{short_hash, GitManager};
use crate::scheduler::Scheduler;
use crate::storage::Storage;
use crate::types::{Engine, TestStatus, TimeControl};

pub struct WebState {
    pub storage: Storage,
    pub config: Config,
}

pub fn create_router(storage: Storage, config: Config) -> Router {
    let state = Arc::new(WebState { storage, config });

    Router::new()
        .route("/", get(index_handler))
        .route("/api/status", get(status_handler))
        .route("/api/engines", get(engines_handler))
        .route("/api/timeline/{engine_id}", get(timeline_handler))
        .route("/api/jobs", get(jobs_handler))
        .route("/api/bisect", get(active_bisect_sessions_handler))
        .route("/api/admin/engines", post(add_engine_handler))
        .route(
            "/api/admin/engines/{engine_id}",
            delete(delete_engine_handler),
        )
        .route("/api/admin/tests", post(queue_manual_test_handler))
        .route("/api/admin/bisect", post(start_bisect_handler))
        .route(
            "/api/admin/bisect/{session_id}/cancel",
            post(cancel_bisect_session_handler),
        )
        .route("/api/admin/jobs/{job_id}/cancel", post(cancel_job_handler))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct AddEngineRequest {
    name: String,
    repo: String,
    branches: Vec<String>,
    build_cmd: String,
    binary_path: String,
    start_from: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ManualTestRequest {
    engine_id: String,
    dev: String,
    base: String,
}

#[derive(Debug, Deserialize)]
struct StartBisectRequest {
    engine_id: String,
    good: String,
    bad: String,
}

async fn index_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn status_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_system_status() {
        Ok(status) => Json(serde_json::to_value(status).unwrap()).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn engines_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_engines() {
        Ok(engines) => Json(serde_json::to_value(engines).unwrap()).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn timeline_handler(
    State(state): State<Arc<WebState>>,
    Path(engine_id): Path<String>,
) -> impl IntoResponse {
    match state.storage.get_elo_timeline(&engine_id, None) {
        Ok(timeline) => Json(serde_json::to_value(timeline).unwrap()).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn jobs_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.list_recent_jobs(50) {
        Ok(jobs) => Json(json!({ "jobs": jobs })).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn active_bisect_sessions_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match state.storage.get_running_bisect_sessions() {
        Ok(sessions) => Json(serde_json::to_value(sessions).unwrap()).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn add_engine_handler(
    State(state): State<Arc<WebState>>,
    Json(request): Json<AddEngineRequest>,
) -> impl IntoResponse {
    let branches = request
        .branches
        .iter()
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty())
        .collect::<Vec<_>>();
    if request.name.trim().is_empty()
        || request.repo.trim().is_empty()
        || request.build_cmd.trim().is_empty()
        || request.binary_path.trim().is_empty()
        || branches.is_empty()
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            anyhow::anyhow!("engine form is incomplete"),
        );
    }

    match create_or_update_engine(&state, request, branches) {
        Ok(engine) => Json(json!({ "engine": engine })).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn delete_engine_handler(
    State(state): State<Arc<WebState>>,
    Path(engine_id): Path<String>,
) -> impl IntoResponse {
    let engine = match state.storage.get_engine_by_id(&engine_id) {
        Ok(engine) => engine,
        Err(err) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    };
    let Some(engine) = engine else {
        return json_error(StatusCode::NOT_FOUND, anyhow::anyhow!("engine not found"));
    };

    match state.storage.delete_engine(&engine_id) {
        Ok(true) => {
            if engine.local_path.exists() {
                if let Err(err) = std::fs::remove_dir_all(&engine.local_path) {
                    warn!(
                        "Failed to remove repo directory '{}': {}",
                        engine.local_path.display(),
                        err
                    );
                }
            }
            Json(json!({ "deleted": true })).into_response()
        }
        Ok(false) => json_error(StatusCode::NOT_FOUND, anyhow::anyhow!("engine not found")),
        Err(err) => json_error(StatusCode::CONFLICT, err),
    }
}

async fn queue_manual_test_handler(
    State(state): State<Arc<WebState>>,
    Json(request): Json<ManualTestRequest>,
) -> impl IntoResponse {
    match queue_manual_test(&state, request) {
        Ok(payload) => Json(payload).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn start_bisect_handler(
    State(state): State<Arc<WebState>>,
    Json(request): Json<StartBisectRequest>,
) -> impl IntoResponse {
    match start_bisect(&state, request) {
        Ok(payload) => Json(payload).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn cancel_job_handler(
    State(state): State<Arc<WebState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let status = match state.storage.get_job_status(&job_id) {
        Ok(Some(status)) => status,
        Ok(None) => {
            return json_error(StatusCode::NOT_FOUND, anyhow::anyhow!("job not found"));
        }
        Err(err) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    };

    match status {
        TestStatus::Queued | TestStatus::Running => match state.storage.cancel_job(&job_id) {
            Ok(true) => {
                let _ = state.storage.mark_bisect_session_failed_for_job(&job_id);
                Json(json!({ "cancelled": true })).into_response()
            }
            Ok(false) => json_error(
                StatusCode::CONFLICT,
                anyhow::anyhow!("job could not be cancelled"),
            ),
            Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
        },
        TestStatus::Completed | TestStatus::Cancelled | TestStatus::Failed => json_error(
            StatusCode::CONFLICT,
            anyhow::anyhow!("job is already {}", format!("{status:?}").to_lowercase()),
        ),
    }
}

async fn cancel_bisect_session_handler(
    State(state): State<Arc<WebState>>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match state.storage.cancel_bisect_session(&session_id) {
        Ok(true) => Json(json!({ "cancelled": true })).into_response(),
        Ok(false) => json_error(
            StatusCode::NOT_FOUND,
            anyhow::anyhow!("bisect session not found or already finished"),
        ),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

fn create_or_update_engine(
    state: &WebState,
    request: AddEngineRequest,
    branches: Vec<String>,
) -> anyhow::Result<Engine> {
    let existing = state.storage.get_engine_by_name(request.name.trim())?;
    let engine = Engine {
        id: existing
            .as_ref()
            .map(|engine| engine.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name: request.name.trim().to_string(),
        repo_url: request.repo.trim().to_string(),
        local_path: state
            .config
            .data_dir
            .join("repos")
            .join(request.name.trim()),
        branches,
        build_cmd: request.build_cmd.trim().to_string(),
        binary_path: request.binary_path.trim().to_string(),
        start_from: request
            .start_from
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    };
    state.storage.insert_engine(&engine)?;
    Ok(engine)
}

fn queue_manual_test(
    state: &WebState,
    request: ManualTestRequest,
) -> anyhow::Result<serde_json::Value> {
    let engine = state
        .storage
        .get_engine_by_id(&request.engine_id)?
        .ok_or_else(|| anyhow::anyhow!("engine not found"))?;
    let git_mgr = GitManager::new(
        &engine.repo_url,
        &engine.local_path,
        &engine.build_cmd,
        &engine.binary_path,
    );
    let repo = git_mgr.ensure_repo()?;
    sync_engine_revisions(state, &engine, &git_mgr, &repo)?;

    let dev_revision = state
        .storage
        .get_revision_by_hash_prefix(&engine.id, request.dev.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve dev commit"))?;
    let base_revision = state
        .storage
        .get_revision_by_hash_prefix(&engine.id, request.base.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve base commit"))?;

    let scheduler = Scheduler::new(state.storage.clone(), state.config.clone());
    let job = scheduler.schedule_manual_test(&engine.id, &dev_revision.id, &base_revision.id)?;

    Ok(json!({
        "job_id": job.id,
        "engine_id": engine.id,
        "engine_name": engine.name,
        "dev_commit": short_hash(&dev_revision.commit_hash),
        "base_commit": short_hash(&base_revision.commit_hash),
    }))
}

fn start_bisect(
    state: &WebState,
    request: StartBisectRequest,
) -> anyhow::Result<serde_json::Value> {
    let engine = state
        .storage
        .get_engine_by_id(&request.engine_id)?
        .ok_or_else(|| anyhow::anyhow!("engine not found"))?;
    let git_mgr = GitManager::new(
        &engine.repo_url,
        &engine.local_path,
        &engine.build_cmd,
        &engine.binary_path,
    );
    let repo = git_mgr.ensure_repo()?;
    sync_engine_revisions(state, &engine, &git_mgr, &repo)?;

    let good_revision = state
        .storage
        .get_revision_by_hash_prefix(&engine.id, request.good.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve good commit"))?;
    let bad_revision = state
        .storage
        .get_revision_by_hash_prefix(&engine.id, request.bad.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve bad commit"))?;

    let bisect_runner = BisectRunner::new(state.storage.clone());
    let mut session = bisect_runner.start_bisect(
        &engine.id,
        &good_revision.id,
        &bad_revision.id,
        git_mgr.commits_between(&repo, &good_revision.commit_hash, &bad_revision.commit_hash)?,
    )?;

    let response = match bisect_runner.next_commit_to_test(&mut session) {
        Some(BisectStep::Test {
            commit_hash,
            remaining_range,
            phase,
            ..
        }) => {
            queue_bisect_probe(
                state,
                &bisect_runner,
                &mut session,
                &engine.id,
                &good_revision.id,
                &commit_hash,
            )?;
            state.storage.insert_bisect_session(&session)?;
            json!({
                "session_id": session.id,
                "engine_id": engine.id,
                "engine_name": engine.name,
                "phase": phase,
                "remaining_range": remaining_range,
                "first_probe": short_hash(&commit_hash),
            })
        }
        Some(BisectStep::Found { culprit }) => json!({
            "session_id": session.id,
            "engine_id": engine.id,
            "engine_name": engine.name,
            "phase": "Found",
            "culprit": culprit,
        }),
        Some(BisectStep::Failed { reason }) => {
            anyhow::bail!("could not start bisect: {}", reason);
        }
        None => {
            anyhow::bail!("no bisect probe was scheduled");
        }
    };

    Ok(response)
}

fn sync_engine_revisions(
    state: &WebState,
    engine: &Engine,
    git_mgr: &GitManager,
    repo: &git2::Repository,
) -> anyhow::Result<()> {
    for branch in &engine.branches {
        let revisions =
            git_mgr.list_commits(repo, branch, &engine.id, engine.start_from.as_deref())?;
        for revision in &revisions {
            state.storage.insert_revision(revision)?;
        }
    }

    let revisions = state.storage.get_revisions_for_engine(&engine.id)?;
    for revision in revisions
        .iter()
        .filter(|revision| revision.build_status == crate::types::BuildStatus::Pending)
    {
        match git_mgr.build_revision(repo, &revision.commit_hash) {
            Ok(binary) => {
                state.storage.update_build_status(
                    &revision.id,
                    crate::types::BuildStatus::Success,
                    Some(&binary),
                )?;
            }
            Err(err) => {
                warn!(
                    "Build failed for {} during web sync: {}",
                    short_hash(&revision.commit_hash),
                    err
                );
                state.storage.update_build_status(
                    &revision.id,
                    crate::types::BuildStatus::Failed,
                    None,
                )?;
            }
        }
    }

    Ok(())
}

fn queue_bisect_probe(
    state: &WebState,
    bisect_runner: &BisectRunner,
    session: &mut crate::types::BisectSession,
    engine_id: &str,
    baseline_revision_id: &str,
    commit_hash: &str,
) -> anyhow::Result<()> {
    let test_revision = state
        .storage
        .get_revision_by_hash_prefix(engine_id, commit_hash)?
        .ok_or_else(|| anyhow::anyhow!("could not resolve bisect probe"))?;
    let job = bisect_runner.create_bisect_job(
        engine_id,
        &test_revision.id,
        baseline_revision_id,
        configured_time_control(&state.config),
    );
    session.current_job_id = Some(job.id.clone());
    state.storage.insert_test_job(&job)?;
    Ok(())
}

fn configured_time_control(config: &Config) -> TimeControl {
    TimeControl {
        base_time_ms: config.testing.time_control.base_ms,
        increment_ms: config.testing.time_control.increment_ms,
        nodes: config.testing.time_control.nodes,
    }
}

fn json_error(status: StatusCode, err: impl std::fmt::Display) -> Response {
    (
        status,
        Json(json!({
            "error": err.to_string(),
        })),
    )
        .into_response()
}

/// The dashboard as a single embedded HTML page.
const DASHBOARD_HTML: &str = include_str!("../../templates/dashboard.html");
