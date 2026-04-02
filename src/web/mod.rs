//! Web server for the Crucible dashboard and admin surface.

use axum::{
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        HeaderMap, StatusCode,
    },
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tracing::warn;

use crate::bisect::{BisectRunner, BisectStep};
use crate::config::Config;
use crate::git::{short_hash, CommitDetails, DiffSummary, GitManager};
use crate::scheduler::Scheduler;
use crate::storage::Storage;
use crate::training::list_training_runs;
use crate::types::{Engine, EngineRevision, JobSummary, TestStatus, TimeControl};

pub struct WebState {
    pub storage: Storage,
    pub config: Config,
}

pub fn create_router(storage: Storage, config: Config) -> Router {
    let state = Arc::new(WebState { storage, config });

    Router::new()
        .route("/", get(index_handler))
        .route("/favicon.svg", get(favicon_handler))
        .route("/api/status", get(status_handler))
        .route("/api/engines", get(engines_handler))
        .route("/api/timeline/:engine_id", get(timeline_handler))
        .route("/api/jobs", get(jobs_handler))
        .route("/api/bisect", get(active_bisect_sessions_handler))
        .route("/api/training/runs", get(training_runs_handler))
        .route(
            "/api/revisions/:engine_id/:revision_ref",
            get(revision_details_handler),
        )
        .route("/api/compare/:engine_id", get(compare_handler))
        .route("/api/admin/engines", post(add_engine_handler))
        .route(
            "/api/admin/engines/:engine_id",
            delete(delete_engine_handler),
        )
        .route("/api/admin/tests", post(queue_manual_test_handler))
        .route("/api/admin/bisect", post(start_bisect_handler))
        .route(
            "/api/admin/bisect/:session_id/cancel",
            post(cancel_bisect_session_handler),
        )
        .route("/api/admin/jobs/:job_id/cancel", post(cancel_job_handler))
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

#[derive(Debug, Deserialize)]
struct CompareQuery {
    base: String,
    head: String,
}

#[derive(Debug, Serialize)]
struct RevisionDetailsResponse {
    engine_id: String,
    engine_name: String,
    revision: EngineRevision,
    commit: CommitDetails,
    compare_to_parent: DiffSummary,
    related_jobs: Vec<JobSummary>,
    lineage: RevisionLineage,
}

#[derive(Debug, Serialize)]
struct CompareResponse {
    engine_id: String,
    engine_name: String,
    base_revision: EngineRevision,
    head_revision: EngineRevision,
    summary: DiffSummary,
    related_jobs: Vec<JobSummary>,
    base_lineage: RevisionLineage,
    head_lineage: RevisionLineage,
}

#[derive(Debug, Serialize, Clone)]
struct LineageRef {
    revision_id: String,
    commit_hash: String,
    commit_message: String,
    branch: String,
    tag: Option<String>,
    binary_fingerprint: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct RevisionLineage {
    branch: String,
    previous_branch_revision: Option<LineageRef>,
    next_branch_revision: Option<LineageRef>,
    previous_distinct_binary_revision: Option<LineageRef>,
    next_distinct_binary_revision: Option<LineageRef>,
    skipped_identical_previous: usize,
    skipped_identical_next: usize,
}

async fn index_handler() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn favicon_handler() -> impl IntoResponse {
    (
        [
            (CONTENT_TYPE, "image/svg+xml"),
            (CACHE_CONTROL, "public, max-age=86400"),
        ],
        FAVICON_SVG,
    )
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

async fn training_runs_handler(State(state): State<Arc<WebState>>) -> impl IntoResponse {
    match list_training_runs(&state.config.training.output_dir) {
        Ok(runs) => Json(json!({ "runs": runs })).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn revision_details_handler(
    State(state): State<Arc<WebState>>,
    Path((engine_id, revision_ref)): Path<(String, String)>,
) -> impl IntoResponse {
    match load_revision_details(&state, &engine_id, &revision_ref) {
        Ok(payload) => Json(serde_json::to_value(payload).unwrap()).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn compare_handler(
    State(state): State<Arc<WebState>>,
    Path(engine_id): Path<String>,
    Query(query): Query<CompareQuery>,
) -> impl IntoResponse {
    match load_compare_details(&state, &engine_id, &query.base, &query.head) {
        Ok(payload) => Json(serde_json::to_value(payload).unwrap()).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn add_engine_handler(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<AddEngineRequest>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
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
    headers: HeaderMap,
    Path(engine_id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
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
    headers: HeaderMap,
    Json(request): Json<ManualTestRequest>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
    match queue_manual_test(&state, request) {
        Ok(payload) => Json(payload).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn start_bisect_handler(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<StartBisectRequest>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
    match start_bisect(&state, request) {
        Ok(payload) => Json(payload).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn cancel_job_handler(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
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
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
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

fn load_revision_details(
    state: &WebState,
    engine_id: &str,
    revision_ref: &str,
) -> anyhow::Result<RevisionDetailsResponse> {
    let engine = state
        .storage
        .get_engine_by_id(engine_id)?
        .ok_or_else(|| anyhow::anyhow!("engine not found"))?;
    let revision = state
        .storage
        .get_revision_by_ref_prefix(&engine.id, revision_ref)?
        .ok_or_else(|| anyhow::anyhow!("could not resolve revision"))?;
    let git_mgr = GitManager::new(
        &engine.repo_url,
        &engine.local_path,
        &engine.build_cmd,
        &engine.binary_path,
    );
    let repo = git_mgr.ensure_repo()?;
    let commit = git_mgr.commit_details(&repo, &revision.commit_hash)?;
    let compare_to_parent = git_mgr.diff_for_revision(&repo, &revision.commit_hash)?;
    let related_jobs = state.storage.list_jobs_for_revision(&revision.id, 8)?;
    let lineage = build_revision_lineage(&state.storage, &engine.id, &revision)?;

    Ok(RevisionDetailsResponse {
        engine_id: engine.id,
        engine_name: engine.name,
        revision,
        commit,
        compare_to_parent,
        related_jobs,
        lineage,
    })
}

fn load_compare_details(
    state: &WebState,
    engine_id: &str,
    base_ref: &str,
    head_ref: &str,
) -> anyhow::Result<CompareResponse> {
    let engine = state
        .storage
        .get_engine_by_id(engine_id)?
        .ok_or_else(|| anyhow::anyhow!("engine not found"))?;
    let base_revision = state
        .storage
        .get_revision_by_ref_prefix(&engine.id, base_ref.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve base revision"))?;
    let head_revision = state
        .storage
        .get_revision_by_ref_prefix(&engine.id, head_ref.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve head revision"))?;

    let git_mgr = GitManager::new(
        &engine.repo_url,
        &engine.local_path,
        &engine.build_cmd,
        &engine.binary_path,
    );
    let repo = git_mgr.ensure_repo()?;
    let summary = git_mgr.diff_between(
        &repo,
        &base_revision.commit_hash,
        &head_revision.commit_hash,
    )?;
    let related_jobs = state.storage.list_jobs_for_revision(&head_revision.id, 8)?;
    let base_lineage = build_revision_lineage(&state.storage, &engine.id, &base_revision)?;
    let head_lineage = build_revision_lineage(&state.storage, &engine.id, &head_revision)?;

    Ok(CompareResponse {
        engine_id: engine.id,
        engine_name: engine.name,
        base_revision,
        head_revision,
        summary,
        related_jobs,
        base_lineage,
        head_lineage,
    })
}

fn build_revision_lineage(
    storage: &Storage,
    engine_id: &str,
    revision: &EngineRevision,
) -> anyhow::Result<RevisionLineage> {
    let mut branch_revisions = storage
        .get_branch_revisions_for_engine(engine_id)?
        .into_iter()
        .filter(|candidate| candidate.branch == revision.branch)
        .collect::<Vec<_>>();
    branch_revisions.sort_by_key(|candidate| candidate.commit_date);

    let index = branch_revisions
        .iter()
        .position(|candidate| candidate.id == revision.id)
        .ok_or_else(|| anyhow::anyhow!("revision not found on branch"))?;

    let previous_branch_revision = index
        .checked_sub(1)
        .and_then(|idx| branch_revisions.get(idx))
        .map(lineage_ref);
    let next_branch_revision = branch_revisions.get(index + 1).map(lineage_ref);

    let mut skipped_identical_previous = 0;
    let mut previous_distinct_binary_revision = None;
    for candidate in branch_revisions[..index].iter().rev() {
        if candidate.binary_fingerprint.is_some()
            && candidate.binary_fingerprint == revision.binary_fingerprint
        {
            skipped_identical_previous += 1;
            continue;
        }
        previous_distinct_binary_revision = Some(lineage_ref(candidate));
        break;
    }

    let mut skipped_identical_next = 0;
    let mut next_distinct_binary_revision = None;
    for candidate in branch_revisions.iter().skip(index + 1) {
        if candidate.binary_fingerprint.is_some()
            && candidate.binary_fingerprint == revision.binary_fingerprint
        {
            skipped_identical_next += 1;
            continue;
        }
        next_distinct_binary_revision = Some(lineage_ref(candidate));
        break;
    }

    Ok(RevisionLineage {
        branch: revision.branch.clone(),
        previous_branch_revision,
        next_branch_revision,
        previous_distinct_binary_revision,
        next_distinct_binary_revision,
        skipped_identical_previous,
        skipped_identical_next,
    })
}

fn lineage_ref(revision: &EngineRevision) -> LineageRef {
    LineageRef {
        revision_id: revision.id.clone(),
        commit_hash: revision.commit_hash.clone(),
        commit_message: revision.commit_message.clone(),
        branch: revision.branch.clone(),
        tag: revision.tag.clone(),
        binary_fingerprint: revision.binary_fingerprint.clone(),
    }
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
                let fingerprint = GitManager::fingerprint_binary(&binary)?;
                state.storage.update_build_status(
                    &revision.id,
                    crate::types::BuildStatus::Success,
                    Some(&binary),
                    Some(&fingerprint),
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

fn authorize_admin(headers: &HeaderMap, state: &WebState) -> Result<(), Response> {
    let Some(expected_token) = state.config.server.admin_token.as_deref() else {
        return Ok(());
    };

    let provided = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);

    match provided {
        Some(token) if token == expected_token => Ok(()),
        _ => Err(json_error(
            StatusCode::UNAUTHORIZED,
            "admin bearer token required",
        )),
    }
}

/// The dashboard as a single embedded HTML page.
const DASHBOARD_HTML: &str = include_str!("../../templates/dashboard.html");
const FAVICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#17212d"/>
      <stop offset="100%" stop-color="#090e14"/>
    </linearGradient>
    <linearGradient id="ember" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0%" stop-color="#ffd782"/>
      <stop offset="55%" stop-color="#ff9a3d"/>
      <stop offset="100%" stop-color="#e4572e"/>
    </linearGradient>
  </defs>
  <rect width="64" height="64" rx="14" fill="url(#bg)"/>
  <path
    d="M45.5 18.8c-3.2-3.4-7.9-5.3-13.1-5.3-9.8 0-17.9 7.5-17.9 18.4 0 10.7 7.7 18.6 18.4 18.6 5.1 0 9.6-1.8 12.7-5.2l-6.1-6.3c-1.8 1.8-4 2.8-6.5 2.8-5.8 0-9.1-4.3-9.1-9.9 0-5.9 3.7-9.8 9-9.8 2.5 0 4.8 1 6.7 3z"
    fill="url(#ember)"
  />
  <path
    d="M18 18.5h26.5l-3.2 5.4H21.2z"
    fill="#fff2cf"
    opacity=".82"
  />
</svg>
"##;
