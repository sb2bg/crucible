use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crucible::bisect::{BisectAction, BisectRunner, BisectStep};
use crucible::config::Config;
use crucible::engine::match_runner::{run_match, MatchConfig};
use crucible::git::{short_hash, GitManager};
use crucible::scheduler::Scheduler;
use crucible::sprt::SprtBounds;
use crucible::storage::Storage;
use crucible::types::{
    BisectStatus, BuildStatus, Engine, JobType, ProbeVerdict, TestJob, TestResult, TestStatus,
    TimeControl,
};

#[derive(Parser)]
#[command(
    name = "crucible",
    about = "⚗  Crucible — CI for chess engines\n\nAutomated SPRT regression testing across your git history.",
    version
)]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "crucible.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the Crucible daemon (continuous testing + web UI)
    Run {
        /// Also launch TUI monitoring
        #[arg(long)]
        tui: bool,
    },

    /// Monitor the running daemon with the terminal UI
    Monitor,

    /// Add an engine to track
    Add {
        /// Engine name
        #[arg(short, long)]
        name: String,
        /// GitHub repository URL
        #[arg(short, long)]
        repo: String,
        /// Build command (e.g., "make" or "cargo build --release")
        #[arg(short, long)]
        build: String,
        /// Path to binary after build, relative to repo root
        #[arg(short = 'p', long)]
        binary_path: String,
        /// Branch(es) to track (comma-separated)
        #[arg(long, default_value = "main")]
        branches: String,
        /// Start from this commit/tag
        #[arg(long)]
        start_from: Option<String>,
    },

    /// List tracked engines
    List,

    /// Start a bisect to find a regression
    Bisect {
        /// Engine name
        #[arg(short, long)]
        engine: String,
        /// Known good commit/tag
        #[arg(long)]
        good: String,
        /// Known bad commit/tag
        #[arg(long)]
        bad: String,
    },

    /// Manually queue a test between two commits
    Test {
        /// Engine name
        #[arg(short, long)]
        engine: String,
        /// Dev commit hash
        #[arg(long)]
        dev: String,
        /// Base commit hash
        #[arg(long)]
        base: String,
    },

    /// Show test results and Elo timeline
    Status {
        /// Engine name (optional, shows all if omitted)
        engine: Option<String>,
    },

    /// Generate an example config file
    Init,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let config = Config::load(&cli.config)?;

    match cli.command {
        Commands::Init => {
            let example = Config::example();
            std::fs::write("crucible.toml", &example)?;
            println!("Created crucible.toml with example configuration.");
            println!("Edit it to add your engine, then run `crucible run`.");
        }

        Commands::Run { tui } => {
            info!("Starting Crucible daemon...");
            let storage = open_storage(&config)?;

            // Start web server
            let web_storage = storage.clone();
            let web_host = config.server.web_host.clone();
            let web_port = config.server.web_port;

            let _web_handle = tokio::spawn(async move {
                let router = crucible::web::create_router(web_storage);
                let addr = format!("{}:{}", web_host, web_port);
                info!("Web dashboard: http://{}", addr);
                let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
                axum::serve(listener, router).await.unwrap();
            });

            // Start the main testing loop
            let main_storage = storage.clone();
            let main_config = config.clone();
            let _test_handle = tokio::spawn(async move {
                run_test_loop(main_storage, main_config).await;
            });

            if tui {
                // Run TUI in the main thread (blocking)
                let mut tui_app = crucible::tui::Tui::new(storage.clone());
                tui_app.run()?;
            } else {
                println!("Crucible is running.");
                println!(
                    "  Web UI:  http://{}:{}",
                    config.server.web_host, config.server.web_port
                );
                println!("  TUI:    crucible monitor");
                println!();
                println!("Press Ctrl+C to stop.");

                // Wait forever (or until Ctrl+C)
                tokio::signal::ctrl_c().await?;
                info!("Shutting down...");
            }
        }

        Commands::Monitor => {
            let storage = open_storage(&config)?;
            let mut tui_app = crucible::tui::Tui::new(storage);
            tui_app.run()?;
        }

        Commands::Add {
            name,
            repo,
            build,
            binary_path,
            branches,
            start_from,
        } => {
            let storage = open_storage(&config)?;
            let existing = storage.get_engine_by_name(&name)?;

            let engine = crucible::types::Engine {
                id: existing
                    .as_ref()
                    .map(|engine| engine.id.clone())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                name: name.clone(),
                repo_url: repo.clone(),
                local_path: config.data_dir.join("repos").join(&name),
                branches: branches.split(',').map(|s| s.trim().to_string()).collect(),
                build_cmd: build,
                binary_path,
                start_from,
            };

            storage.insert_engine(&engine)?;
            println!("Added engine '{}' tracking {}", name, repo);
            println!("Run `crucible run` to start testing.");
        }

        Commands::List => {
            let storage = open_storage(&config)?;
            let engines = storage.get_engines()?;

            if engines.is_empty() {
                println!("No engines tracked. Run `crucible add` first.");
            } else {
                println!("{:<20} {:<40} {}", "NAME", "REPO", "BRANCHES");
                println!("{}", "─".repeat(80));
                for e in &engines {
                    println!(
                        "{:<20} {:<40} {}",
                        e.name,
                        e.repo_url,
                        e.branches.join(", ")
                    );
                }
            }
        }

        Commands::Bisect { engine, good, bad } => {
            let storage = open_storage(&config)?;
            let engine = storage
                .get_engine_by_name(&engine)?
                .with_context(|| format!("Engine '{}' is not tracked", engine))?;
            let git_mgr = GitManager::new(
                &engine.repo_url,
                &engine.local_path,
                &engine.build_cmd,
                &engine.binary_path,
            );
            let repo = git_mgr.ensure_repo()?;
            sync_engine_revisions(&storage, &engine, &git_mgr, &repo)?;

            let good_revision = storage
                .get_revision_by_hash_prefix(&engine.id, &good)?
                .with_context(|| format!("Could not resolve good commit '{}'", good))?;
            let bad_revision = storage
                .get_revision_by_hash_prefix(&engine.id, &bad)?
                .with_context(|| format!("Could not resolve bad commit '{}'", bad))?;

            let bisect_runner = BisectRunner::new(storage.clone());
            let mut session = bisect_runner.start_bisect(
                &engine.id,
                &good_revision.id,
                &bad_revision.id,
                git_mgr.commits_between(
                    &repo,
                    &good_revision.commit_hash,
                    &bad_revision.commit_hash,
                )?,
            )?;

            match bisect_runner.next_commit_to_test(&mut session) {
                Some(BisectStep::Test {
                    commit_hash,
                    remaining_range,
                    ..
                }) => {
                    queue_bisect_probe(
                        &storage,
                        &bisect_runner,
                        &mut session,
                        &engine.id,
                        &good_revision.id,
                        &commit_hash,
                        &config,
                    )?;
                    storage.insert_bisect_session(&session)?;

                    println!(
                        "Queued regression hunt for '{}' over {} commits.",
                        engine.name, remaining_range
                    );
                    println!("First probe: {}", short_hash(&commit_hash));
                }
                Some(BisectStep::Found { culprit }) => {
                    println!(
                        "Range is already minimal. Suspected culprit revision: {}",
                        culprit
                    );
                }
                Some(BisectStep::Failed { reason }) => {
                    println!("Could not start regression hunt: {}", reason);
                }
                None => {
                    println!("No regression-hunt probe was scheduled.");
                }
            }
        }

        Commands::Test { engine, dev, base } => {
            let storage = open_storage(&config)?;
            let engine = storage
                .get_engine_by_name(&engine)?
                .with_context(|| format!("Engine '{}' is not tracked", engine))?;
            let git_mgr = GitManager::new(
                &engine.repo_url,
                &engine.local_path,
                &engine.build_cmd,
                &engine.binary_path,
            );
            let repo = git_mgr.ensure_repo()?;
            sync_engine_revisions(&storage, &engine, &git_mgr, &repo)?;

            let dev_revision = storage
                .get_revision_by_hash_prefix(&engine.id, &dev)?
                .with_context(|| format!("Could not resolve dev commit '{}'", dev))?;
            let base_revision = storage
                .get_revision_by_hash_prefix(&engine.id, &base)?
                .with_context(|| format!("Could not resolve base commit '{}'", base))?;

            let scheduler = Scheduler::new(storage.clone(), config.clone());
            let job =
                scheduler.schedule_manual_test(&engine.id, &dev_revision.id, &base_revision.id)?;

            println!(
                "Queued manual test: {} vs {} for '{}'",
                short_hash(&dev_revision.commit_hash),
                short_hash(&base_revision.commit_hash),
                engine.name
            );
            println!("Job id: {}", job.id);
        }

        Commands::Status { engine: _ } => {
            let storage = open_storage(&config)?;
            let status = storage.get_system_status()?;

            println!("⚗  Crucible Status");
            println!("═══════════════════════════════════");
            println!("  Engines:     {}", status.engines_tracked);
            println!("  Active:      {}", status.active_jobs);
            println!("  Queued:      {}", status.queued_jobs);
            println!("  Completed:   {}", status.completed_jobs);
            println!("  Games:       {}", status.total_games_played);
        }
    }

    Ok(())
}

/// The main continuous testing loop
async fn run_test_loop(storage: Storage, config: Config) {
    info!("Test loop started");
    let worker_count = usize::try_from(config.testing.concurrency.max(1)).unwrap_or(1);

    loop {
        // 1. For each tracked engine, fetch latest commits
        let engines = match storage.get_engines() {
            Ok(e) => e,
            Err(err) => {
                tracing::error!("Failed to get engines: {}", err);
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                continue;
            }
        };

        for engine in &engines {
            // Clone/fetch repo, enumerate commits, build, schedule tests
            let git_mgr = GitManager::new(
                &engine.repo_url,
                &engine.local_path,
                &engine.build_cmd,
                &engine.binary_path,
            );

            match git_mgr.ensure_repo() {
                Ok(repo) => {
                    if let Err(e) = sync_engine_revisions(&storage, engine, &git_mgr, &repo) {
                        tracing::error!("Failed to sync revisions for '{}': {}", engine.name, e);
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to access repo for '{}': {}", engine.name, e);
                }
            }

            // Schedule and run test jobs
            let scheduler = Scheduler::new(storage.clone(), config.clone());
            match scheduler.schedule_engine(&engine.id) {
                Ok(jobs) => {
                    for job in &jobs {
                        let _ = storage.insert_test_job(job);
                    }
                    if !jobs.is_empty() {
                        info!(
                            "Scheduled {} new test jobs for '{}'",
                            jobs.len(),
                            engine.name
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("Scheduler error for '{}': {}", engine.name, e);
                }
            }
        }

        // 2. Process queued jobs
        loop {
            let mut batch = Vec::new();
            while batch.len() < worker_count {
                match storage.claim_next_job() {
                    Ok(Some(job)) => batch.push(job),
                    Ok(None) => break,
                    Err(err) => {
                        tracing::error!("Failed to claim next job: {}", err);
                        break;
                    }
                }
            }

            if batch.is_empty() {
                break;
            }

            let mut handles = Vec::with_capacity(batch.len());
            for job in batch {
                let job_storage = storage.clone();
                let job_config = config.clone();
                handles.push(tokio::spawn(async move {
                    process_claimed_job(job_storage, job_config, job).await;
                }));
            }

            for handle in handles {
                if let Err(err) = handle.await {
                    tracing::error!("Job worker task panicked: {}", err);
                }
            }
        }

        // Sleep before next polling cycle
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

fn open_storage(config: &Config) -> Result<Storage> {
    std::fs::create_dir_all(&config.data_dir)?;
    let db_path = config.data_dir.join("crucible.db");
    let storage = Storage::open(&db_path)?;
    sync_config_engines(&storage, config)?;
    Ok(storage)
}

fn sync_config_engines(storage: &Storage, config: &Config) -> Result<()> {
    for engine_cfg in &config.engines {
        let existing = storage.get_engine_by_name(&engine_cfg.name)?;
        let engine = Engine {
            id: existing
                .as_ref()
                .map(|engine| engine.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name: engine_cfg.name.clone(),
            repo_url: engine_cfg.repo.clone(),
            local_path: config.data_dir.join("repos").join(&engine_cfg.name),
            branches: engine_cfg.branches.clone(),
            build_cmd: engine_cfg.build_cmd.clone(),
            binary_path: engine_cfg.binary_path.clone(),
            start_from: engine_cfg.start_from.clone(),
        };
        storage.insert_engine(&engine)?;
    }
    Ok(())
}

async fn process_claimed_job(storage: Storage, config: Config, job: TestJob) {
    info!(
        "Running job {} (dev={}, base={})",
        job.id, job.dev_revision_id, job.base_revision_id
    );

    match execute_job(&storage, &config, &job).await {
        Ok(result) => {
            if let Err(err) = persist_job_result(&storage, &job, &result) {
                tracing::error!("Failed to persist job {}: {}", job.id, err);
                let _ = storage.set_job_status(&job.id, TestStatus::Failed);
                let _ = mark_bisect_session_failed(&storage, &job);
                return;
            }

            if let Err(err) = advance_bisect_after_job(&storage, &config, &job, &result) {
                tracing::error!("Failed to advance bisect for job {}: {}", job.id, err);
                let _ = mark_bisect_session_failed(&storage, &job);
            }
        }
        Err(err) => {
            tracing::error!("Job {} failed: {}", job.id, err);
            let _ = storage.set_job_status(&job.id, TestStatus::Failed);
            let _ = mark_bisect_session_failed(&storage, &job);
        }
    }
}

fn configured_time_control(config: &Config) -> TimeControl {
    TimeControl {
        base_time_ms: config.testing.time_control.base_ms,
        increment_ms: config.testing.time_control.increment_ms,
        nodes: config.testing.time_control.nodes,
    }
}

fn configured_sprt_bounds(config: &Config, job_type: JobType) -> SprtBounds {
    match job_type {
        JobType::Bisect => SprtBounds::regression(),
        _ => SprtBounds {
            elo0: config.testing.sprt.elo0,
            elo1: config.testing.sprt.elo1,
            alpha: config.testing.sprt.alpha,
            beta: config.testing.sprt.beta,
        },
    }
}

fn sync_engine_revisions(
    storage: &Storage,
    engine: &Engine,
    git_mgr: &GitManager,
    repo: &git2::Repository,
) -> Result<()> {
    for branch in &engine.branches {
        let revisions =
            git_mgr.list_commits(repo, branch, &engine.id, engine.start_from.as_deref())?;
        info!(
            "Engine '{}' branch '{}': {} commits",
            engine.name,
            branch,
            revisions.len()
        );
        for revision in &revisions {
            storage.insert_revision(revision)?;
        }
    }

    let revisions = storage.get_revisions_for_engine(&engine.id)?;
    for revision in revisions
        .iter()
        .filter(|r| r.build_status == BuildStatus::Pending)
    {
        match git_mgr.build_revision(repo, &revision.commit_hash) {
            Ok(binary) => {
                storage.update_build_status(&revision.id, BuildStatus::Success, Some(&binary))?;
            }
            Err(err) => {
                warn!(
                    "Build failed for {}: {}",
                    short_hash(&revision.commit_hash),
                    err
                );
                storage.update_build_status(&revision.id, BuildStatus::Failed, None)?;
            }
        }
    }

    Ok(())
}

async fn execute_job(storage: &Storage, config: &Config, job: &TestJob) -> Result<TestResult> {
    let dev_revision = storage
        .get_revision_by_id(&job.dev_revision_id)?
        .with_context(|| format!("Missing dev revision '{}'", job.dev_revision_id))?;
    let base_revision = storage
        .get_revision_by_id(&job.base_revision_id)?
        .with_context(|| format!("Missing base revision '{}'", job.base_revision_id))?;

    let dev_binary = dev_revision
        .binary_path
        .clone()
        .with_context(|| format!("Revision '{}' is missing a built binary", dev_revision.id))?;
    let base_binary = base_revision
        .binary_path
        .clone()
        .with_context(|| format!("Revision '{}' is missing a built binary", base_revision.id))?;

    let (event_tx, _event_rx) = mpsc::unbounded_channel();
    configured_sprt_bounds(config, job.job_type).validate()?;
    run_match(
        MatchConfig {
            dev_binary,
            base_binary,
            time_control: job.time_control.clone(),
            opening_book: load_opening_book(
                job.opening_book
                    .as_deref()
                    .or(config.testing.opening_book.as_deref()),
            )?,
            sprt_bounds: configured_sprt_bounds(config, job.job_type),
            max_games: config.testing.max_games,
            hash_mb: config.testing.hash_mb,
            threads: config.testing.engine_threads,
        },
        event_tx,
    )
    .await
}

fn load_opening_book(path: Option<&str>) -> Result<Option<Vec<String>>> {
    let Some(path) = path else {
        return Ok(None);
    };

    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read opening book '{}'", path))?;
    let openings = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    if openings.is_empty() {
        Ok(None)
    } else {
        Ok(Some(openings))
    }
}

fn persist_job_result(storage: &Storage, job: &TestJob, result: &TestResult) -> Result<()> {
    storage.update_job_result(
        &job.id,
        result.wins,
        result.losses,
        result.draws,
        result.elo_diff,
        result.elo_error,
        result.los,
        result.sprt_result,
    )?;

    for game in &result.games {
        storage.insert_game(&job.id, game)?;
    }

    storage.set_job_status(&job.id, TestStatus::Completed)?;
    Ok(())
}

fn advance_bisect_after_job(
    storage: &Storage,
    config: &Config,
    job: &TestJob,
    result: &TestResult,
) -> Result<()> {
    if job.job_type != JobType::Bisect {
        return Ok(());
    }

    let sessions = storage.get_running_bisect_sessions()?;
    let Some(mut session) = sessions
        .into_iter()
        .find(|session| session.current_job_id.as_deref() == Some(job.id.as_str()))
    else {
        return Ok(());
    };
    let bisect_runner = BisectRunner::new(storage.clone());

    let verdict = match result.sprt_result {
        crucible::types::SprtResult::H1Accepted => ProbeVerdict::Good,
        crucible::types::SprtResult::H0Accepted => ProbeVerdict::Bad,
        crucible::types::SprtResult::Inconclusive => ProbeVerdict::Uncertain,
    };
    let action = bisect_runner.process_result(&mut session, &job.dev_revision_id, &job.id, verdict);

    match action {
        BisectAction::Found { culprit } => {
            session.status = BisectStatus::Found;
            session.current_index = None;
            session.current_job_id = None;
            session.culprit_revision_id = Some(culprit);
            storage.update_bisect_session(&session)?;
        }
        BisectAction::TestNext {
            commit_hash,
            remaining,
            phase,
            ..
        } => {
            let engine_id = session.engine_id.clone();
            let baseline_revision_id = session.good_revision_id.clone();
            queue_bisect_probe(
                storage,
                &bisect_runner,
                &mut session,
                &engine_id,
                &baseline_revision_id,
                &commit_hash,
                config,
            )?;
            storage.update_bisect_session(&session)?;
            info!(
                "Queued next regression-hunt probe for engine {} ({:?}, {} commits in window)",
                session.engine_id, phase, remaining
            );
        }
        BisectAction::Failed { reason } => {
            session.status = BisectStatus::Failed;
            session.current_index = None;
            session.current_job_id = None;
            storage.update_bisect_session(&session)?;
            warn!(
                "Regression hunt failed for engine {}: {}",
                session.engine_id, reason
            );
        }
    }

    Ok(())
}

fn mark_bisect_session_failed(storage: &Storage, job: &TestJob) -> Result<()> {
    if job.job_type != JobType::Bisect {
        return Ok(());
    }

    let sessions = storage.get_running_bisect_sessions()?;
    if let Some(mut session) = sessions
        .into_iter()
        .find(|session| session.current_job_id.as_deref() == Some(job.id.as_str()))
    {
        session.status = BisectStatus::Failed;
        session.current_index = None;
        session.current_job_id = None;
        storage.update_bisect_session(&session)?;
    }

    Ok(())
}

fn queue_bisect_probe(
    storage: &Storage,
    bisect_runner: &BisectRunner,
    session: &mut crucible::types::BisectSession,
    engine_id: &str,
    baseline_revision_id: &str,
    commit_hash: &str,
    config: &Config,
) -> Result<()> {
    let test_revision = storage
        .get_revision_by_hash_prefix(engine_id, commit_hash)?
        .with_context(|| format!("Could not resolve bisect probe '{}'", commit_hash))?;
    let job = bisect_runner.create_bisect_job(
        engine_id,
        &test_revision.id,
        baseline_revision_id,
        configured_time_control(config),
    );
    session.current_job_id = Some(job.id.clone());
    storage.insert_test_job(&job)?;
    Ok(())
}
