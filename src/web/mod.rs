//! Web server for the Crucible dashboard and admin surface.

use axum::{
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, RwLock};
use tracing::warn;

use crate::bisect::{BisectRunner, BisectStep};
use crate::config::Config;
use crate::export::build_export_bundle;
use crate::gate::{
    default_gate_output_path, gate_profile_summaries, list_gate_runs, resolve_gate_profile,
    run_release_gate, write_gate_summary,
};
use crate::git::{branch_pattern_matches, short_hash, CommitDetails, DiffSummary, GitManager};
use crate::scheduler::Scheduler;
use crate::storage::Storage;
use crate::training::list_training_runs;
use crate::types::{Engine, EngineRevision, JobSummary, TestStatus};
use crate::workflow::{queue_bisect_probe, sync_engine_revisions};

pub struct WebState {
    pub storage: Storage,
    pub config: Arc<RwLock<Config>>,
}

pub fn create_router(storage: Storage, config: Arc<RwLock<Config>>) -> Router {
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
        .route("/api/admin/export", get(export_bundle_handler))
        .route(
            "/api/admin/gates",
            get(list_gate_runs_handler).post(start_gate_handler),
        )
        .route(
            "/api/admin/gates/:file_name",
            get(download_gate_run_handler),
        )
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
    #[serde(default)]
    experimental_branches: Vec<String>,
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
struct StartGateRequest {
    engine_id: String,
    candidate: String,
    baseline: String,
    profile: String,
}

#[derive(Debug, Deserialize)]
struct CompareQuery {
    base: String,
    head: String,
}

#[derive(Debug, Deserialize, Default)]
struct LaneQuery {
    lane: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchLane {
    Canonical,
    Experimental,
    All,
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

impl BranchLane {
    fn from_query(value: Option<&str>) -> Self {
        match value.unwrap_or("canonical") {
            "experimental" => Self::Experimental,
            "all" => Self::All,
            _ => Self::Canonical,
        }
    }

    fn includes_branch(self, engine: &Engine, branch: &str) -> bool {
        match self {
            Self::All => true,
            Self::Canonical => !engine_branch_is_experimental(engine, branch),
            Self::Experimental => engine_branch_is_experimental(engine, branch),
        }
    }
}

fn current_config(state: &WebState) -> Config {
    state.config.read().expect("shared config poisoned").clone()
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
    Query(query): Query<LaneQuery>,
) -> impl IntoResponse {
    let lane = BranchLane::from_query(query.lane.as_deref());
    let engine = match state.storage.get_engine_by_id(&engine_id) {
        Ok(Some(engine)) => engine,
        Ok(None) => {
            return json_error(StatusCode::NOT_FOUND, anyhow::anyhow!("engine not found"));
        }
        Err(err) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    };
    match state.storage.get_elo_timeline(&engine_id, None) {
        Ok(timeline) => Json(
            serde_json::to_value(
                timeline
                    .into_iter()
                    .filter(|point| lane.includes_branch(&engine, &point.branch))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )
        .into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn jobs_handler(
    State(state): State<Arc<WebState>>,
    Query(query): Query<LaneQuery>,
) -> impl IntoResponse {
    let lane = BranchLane::from_query(query.lane.as_deref());
    match state.storage.list_all_jobs() {
        Ok(jobs) => match filter_jobs_for_lane(&state, jobs, lane) {
            Ok(filtered) => Json(json!({
                "jobs": filtered.into_iter().take(50).collect::<Vec<_>>()
            }))
            .into_response(),
            Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
        },
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
    let config = current_config(&state);
    match list_training_runs(&config.training.output_dir) {
        Ok(runs) => Json(json!({ "runs": runs })).into_response(),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    }
}

async fn list_gate_runs_handler(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
    let config = current_config(&state);
    match list_gate_runs(&config.data_dir) {
        Ok(runs) => Json(json!({
            "profiles": gate_profile_summaries(&config),
            "runs": runs
        }))
        .into_response(),
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

async fn download_gate_run_handler(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Path(file_name): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }
    if file_name.contains('/') || file_name.contains('\\') || !file_name.ends_with(".json") {
        return json_error(
            StatusCode::BAD_REQUEST,
            anyhow::anyhow!("invalid gate result file name"),
        );
    }
    let config = current_config(&state);
    let path = config.data_dir.join("gates").join(&file_name);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert(
                "content-disposition",
                HeaderValue::from_str(&format!("attachment; filename=\"{}\"", file_name))
                    .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
            );
            (headers, bytes).into_response()
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => json_error(
            StatusCode::NOT_FOUND,
            anyhow::anyhow!("gate result not found"),
        ),
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
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
    let experimental_branches = request
        .experimental_branches
        .iter()
        .map(|branch| branch.trim().to_string())
        .filter(|branch| !branch.is_empty())
        .collect::<Vec<_>>();
    if request.name.trim().is_empty()
        || request.repo.trim().is_empty()
        || request.build_cmd.trim().is_empty()
        || request.binary_path.trim().is_empty()
        || (branches.is_empty() && experimental_branches.is_empty())
    {
        return json_error(
            StatusCode::BAD_REQUEST,
            anyhow::anyhow!("engine form is incomplete"),
        );
    }

    match create_or_update_engine(&state, request, branches, experimental_branches) {
        Ok(engine) => Json(json!({ "engine": engine })).into_response(),
        Err(err) => json_error(StatusCode::BAD_REQUEST, err),
    }
}

async fn start_gate_handler(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
    Json(request): Json<StartGateRequest>,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }

    let config = current_config(&state);
    let engine = match state.storage.get_engine_by_id(&request.engine_id) {
        Ok(Some(engine)) => engine,
        Ok(None) => {
            return json_error(StatusCode::NOT_FOUND, anyhow::anyhow!("engine not found"));
        }
        Err(err) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    };
    let candidate = match state
        .storage
        .get_revision_by_ref_prefix(&engine.id, request.candidate.trim())
    {
        Ok(Some(revision)) => revision,
        Ok(None) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                anyhow::anyhow!("could not resolve candidate revision"),
            );
        }
        Err(err) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    };
    let baseline = match state
        .storage
        .get_revision_by_ref_prefix(&engine.id, request.baseline.trim())
    {
        Ok(Some(revision)) => revision,
        Ok(None) => {
            return json_error(
                StatusCode::BAD_REQUEST,
                anyhow::anyhow!("could not resolve baseline revision"),
            );
        }
        Err(err) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
    };
    let profile = match resolve_gate_profile(&config, request.profile.trim()) {
        Ok(profile) => profile.clone(),
        Err(err) => return json_error(StatusCode::BAD_REQUEST, err),
    };

    let profile_name = profile.name.clone();
    let output_path = default_gate_output_path(&config.data_dir, &profile_name);
    let file_name = output_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("gate.json")
        .to_string();
    let engine_for_task = engine.clone();
    let candidate_for_task = candidate.clone();
    let baseline_for_task = baseline.clone();
    tokio::spawn(async move {
        match run_release_gate(
            &config,
            &engine_for_task,
            &candidate_for_task,
            &baseline_for_task,
            &profile,
        )
        .await
        {
            Ok(summary) => {
                if let Err(err) = write_gate_summary(&output_path, &summary) {
                    tracing::error!("Failed to write gate summary to {:?}: {}", output_path, err);
                }
            }
            Err(err) => {
                tracing::error!(
                    "Gate run failed for '{}' candidate {} baseline {} profile {}: {}",
                    engine_for_task.name,
                    short_hash(&candidate_for_task.commit_hash),
                    short_hash(&baseline_for_task.commit_hash),
                    profile.name,
                    err
                );
            }
        }
    });

    Json(json!({
        "started": true,
        "file_name": file_name,
        "engine_name": engine.name,
        "candidate": candidate.commit_hash,
        "baseline": baseline.commit_hash,
        "profile": profile_name
    }))
    .into_response()
}

async fn export_bundle_handler(
    State(state): State<Arc<WebState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Err(response) = authorize_admin(&headers, &state) {
        return response;
    }

    let config = current_config(&state);
    match build_export_bundle(&state.storage, &config) {
        Ok(bundle) => {
            let filename = format!(
                "crucible-export-{}.json",
                chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
            );
            match serde_json::to_vec_pretty(&bundle) {
                Ok(body) => (
                    [
                        (CONTENT_TYPE, HeaderValue::from_static("application/json")),
                        (
                            axum::http::header::CONTENT_DISPOSITION,
                            HeaderValue::from_str(&format!(
                                "attachment; filename=\"{}\"",
                                filename
                            ))
                            .unwrap_or_else(|_| {
                                HeaderValue::from_static(
                                    "attachment; filename=\"crucible-export.json\"",
                                )
                            }),
                        ),
                        (CACHE_CONTROL, HeaderValue::from_static("no-store")),
                    ],
                    body,
                )
                    .into_response(),
                Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
            }
        }
        Err(err) => json_error(StatusCode::INTERNAL_SERVER_ERROR, err),
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
    experimental_branches: Vec<String>,
) -> anyhow::Result<Engine> {
    let existing = state.storage.get_engine_by_name(request.name.trim())?;
    let config = current_config(state);
    let engine = Engine {
        id: existing
            .as_ref()
            .map(|engine| engine.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        name: request.name.trim().to_string(),
        repo_url: request.repo.trim().to_string(),
        local_path: config.data_dir.join("repos").join(request.name.trim()),
        branches,
        experimental_branches,
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

fn engine_branch_is_experimental(engine: &Engine, branch: &str) -> bool {
    engine
        .experimental_branches
        .iter()
        .any(|pattern| branch_pattern_matches(pattern, branch))
}

fn job_matches_lane(state: &WebState, job: &JobSummary, lane: BranchLane) -> anyhow::Result<bool> {
    if lane == BranchLane::All {
        return Ok(true);
    }

    let engine = state
        .storage
        .get_engine_by_id(&job.engine_id)?
        .ok_or_else(|| anyhow::anyhow!("engine not found for job {}", job.id))?;

    if let Some(branch) = job.branch_context.as_deref() {
        return Ok(lane.includes_branch(&engine, branch));
    }

    let branches = state.storage.get_revision_branches(&job.dev_revision_id)?;

    Ok(branches
        .iter()
        .any(|branch| lane.includes_branch(&engine, branch)))
}

fn filter_jobs_for_lane(
    state: &WebState,
    jobs: Vec<JobSummary>,
    lane: BranchLane,
) -> anyhow::Result<Vec<JobSummary>> {
    let mut filtered = Vec::new();
    for job in jobs {
        if job_matches_lane(state, &job, lane)? {
            filtered.push(job);
        }
    }
    Ok(filtered)
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
    sync_engine_revisions(&state.storage, &engine, &git_mgr, &repo)?;

    let dev_revision = state
        .storage
        .get_revision_by_hash_prefix(&engine.id, request.dev.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve dev commit"))?;
    let base_revision = state
        .storage
        .get_revision_by_hash_prefix(&engine.id, request.base.trim())?
        .ok_or_else(|| anyhow::anyhow!("could not resolve base commit"))?;

    let config = current_config(state);
    let scheduler = Scheduler::new(state.storage.clone(), config);
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
    sync_engine_revisions(&state.storage, &engine, &git_mgr, &repo)?;

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
                &state.storage,
                &bisect_runner,
                &mut session,
                &engine.id,
                &good_revision.id,
                &commit_hash,
                &current_config(state),
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
    let config = current_config(state);
    let Some(expected_token) = config.server.admin_token.as_deref() else {
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
