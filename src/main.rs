use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crucible::bisect::{BisectAction, BisectRunner, BisectStep};
use crucible::chess_rules::load_opening_book;
use crucible::config::Config;
use crucible::engine::match_runner::{
    run_match, MatchConfig, MatchEvent, TaggedTrainingSample, TrainingSampleSource,
};
use crucible::export::build_export_bundle;
use crucible::gate::{
    default_gate_output_path, resolve_gate_profile, run_release_gate, write_gate_summary,
};
use crucible::git::{short_hash, GitManager};
use crucible::scheduler::Scheduler;
use crucible::sprt::SprtBounds;
use crucible::sprt::{elo_error, los, wdl_to_elo};
use crucible::storage::Storage;
use crucible::training::{
    run_selfplay_data_generation, SelfPlayDataConfig, TrainingRunDescriptor, TrainingRunKind,
    TrainingRunStatus, TrainingRunWriter,
};
use crucible::types::{
    BisectStatus, BuildStatus, Engine, JobType, ProbeVerdict, TestJob, TestResult, TestStatus,
};
use crucible::workflow::{configured_time_control, queue_bisect_probe, sync_engine_revisions};

type SharedConfig = Arc<RwLock<Config>>;

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
        /// Experimental branch(es) to track separately (comma-separated)
        #[arg(long, default_value = "")]
        experimental_branches: String,
        /// Start from this commit/tag
        #[arg(long)]
        start_from: Option<String>,
    },

    /// List tracked engines
    List,

    /// Remove a tracked engine
    Remove {
        /// Engine name
        #[arg(short, long)]
        name: String,
        /// Also delete the cloned repo and build artifacts under the data dir
        #[arg(long)]
        delete_data: bool,
    },

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

    /// Generate self-play data for NNUE-style training
    SelfplayData {
        /// Engine name
        #[arg(short, long)]
        engine: String,
        /// Revision hash/tag prefix (default: latest successfully built revision)
        #[arg(long)]
        revision: Option<String>,
        /// Number of self-play games
        #[arg(long)]
        games: Option<u32>,
        /// Override training output directory
        #[arg(long)]
        output_dir: Option<PathBuf>,
        /// Exact reported search depth to keep in the exported dataset
        #[arg(long, alias = "min-depth")]
        depth: Option<u32>,
    },

    /// Run a fixed gauntlet gate: candidate and baseline vs the same external suite
    Gate {
        /// Engine name
        #[arg(short, long)]
        engine: String,
        /// Candidate revision hash/tag prefix
        #[arg(long)]
        candidate: String,
        /// Baseline revision hash/tag prefix
        #[arg(long)]
        baseline: String,
        /// Gate profile name from config
        #[arg(long)]
        profile: String,
        /// Output JSON file path (default: data_dir/gates/<timestamp>-<profile>.json)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Show test results and Elo timeline
    Status {
        /// Engine name (optional, shows all if omitted)
        engine: Option<String>,
    },

    /// Export engine-testing data as a single JSON bundle
    Export {
        /// Output file path (default: timestamped json in current directory)
        #[arg(short, long)]
        output: Option<PathBuf>,
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
    let shared_config: SharedConfig = Arc::new(RwLock::new(config.clone()));

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
            let web_config = shared_config.clone();
            let _web_handle = tokio::spawn(async move {
                let router = crucible::web::create_router(web_storage, web_config);
                let addr = format!("{}:{}", web_host, web_port);
                info!("Web dashboard: http://{}", addr);
                let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
                axum::serve(listener, router).await.unwrap();
            });

            // Start the main testing loop
            let main_storage = storage.clone();
            let main_config = shared_config.clone();
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
            experimental_branches,
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
                branches: branches
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
                experimental_branches: experimental_branches
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
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

        Commands::Remove { name, delete_data } => {
            let storage = open_storage(&config)?;
            let engine = storage
                .get_engine_by_name(&name)?
                .with_context(|| format!("Engine '{}' is not tracked", name))?;
            let local_path = engine.local_path.clone();
            let removed = storage.delete_engine(&engine.id)?;

            if !removed {
                anyhow::bail!("Engine '{}' is not tracked", name);
            }

            if delete_data && local_path.exists() {
                std::fs::remove_dir_all(&local_path).with_context(|| {
                    format!(
                        "Failed to remove engine data directory '{}'",
                        local_path.display()
                    )
                })?;
            }

            println!("Removed engine '{}'.", name);
            if delete_data {
                println!("Deleted local data at {}.", local_path.display());
            } else {
                println!(
                    "Local repo/build data was kept at {}. Re-run with --delete-data to remove it.",
                    local_path.display()
                );
            }
            if config
                .engines
                .iter()
                .any(|engine_cfg| engine_cfg.name == name)
            {
                println!(
                    "Note: '{}' is still present in the config file and will be re-imported on the next run.",
                    name
                );
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

        Commands::SelfplayData {
            engine,
            revision,
            games,
            output_dir,
            depth,
        } => {
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

            let revision = resolve_selfplay_revision(&storage, &engine, revision.as_deref())?;
            let binary_path = revision.binary_path.clone().with_context(|| {
                format!(
                    "Revision '{}' does not have a built binary",
                    revision.commit_hash
                )
            })?;
            let summary = run_selfplay_data_generation(SelfPlayDataConfig {
                engine_id: engine.id.clone(),
                engine_name: engine.name.clone(),
                revision_id: revision.id.clone(),
                revision_hash: revision.commit_hash.clone(),
                binary_path,
                time_control: configured_time_control(&config),
                opening_book: load_opening_book(config.testing.opening_book.as_deref())?,
                games: games.unwrap_or(config.training.selfplay_games),
                hash_mb: config.testing.hash_mb,
                threads: config.testing.engine_threads,
                output_dir: output_dir.unwrap_or_else(|| config.training.output_dir.clone()),
                depth: depth.unwrap_or(config.training.selfplay_depth),
                kind: TrainingRunKind::SelfPlay,
            })?;

            println!("Generated self-play data for '{}'", engine.name);
            println!("  Revision: {}", short_hash(&revision.commit_hash));
            println!("  Games:    {}", summary.games_played);
            println!("  Samples:  {}", summary.samples_written);
            println!("  Output:   {}", summary.run_dir.display());
            if !summary.depth_counts.is_empty() {
                println!("  Depth buckets:");
                for (depth, count) in summary.depth_counts {
                    println!("    d{:>3}: {}", depth, count);
                }
            }
        }

        Commands::Gate {
            engine,
            candidate,
            baseline,
            profile,
            output,
        } => {
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

            let candidate_revision = resolve_engine_revision(&storage, &engine, &candidate)?;
            let baseline_revision = resolve_engine_revision(&storage, &engine, &baseline)?;
            let gate_profile = resolve_gate_profile(&config, &profile)?;

            let summary = run_release_gate(
                &config,
                &engine,
                &candidate_revision,
                &baseline_revision,
                gate_profile,
            )
            .await?;

            let output =
                output.unwrap_or_else(|| default_gate_output_path(&config.data_dir, &profile));
            write_gate_summary(&output, &summary)?;

            println!("Release gate complete for '{}'", engine.name);
            println!(
                "  Candidate: {}  W{} D{} L{}  {:.2}%  Elo {:+.1} +/- {:.1}  LOS {:.1}%",
                short_hash(&candidate_revision.commit_hash),
                summary.candidate.wins,
                summary.candidate.draws,
                summary.candidate.losses,
                summary.candidate.score_pct,
                summary.candidate.elo_diff,
                summary.candidate.elo_error,
                summary.candidate.los * 100.0,
            );
            println!(
                "  Baseline:  {}  W{} D{} L{}  {:.2}%  Elo {:+.1} +/- {:.1}  LOS {:.1}%",
                short_hash(&baseline_revision.commit_hash),
                summary.baseline.wins,
                summary.baseline.draws,
                summary.baseline.losses,
                summary.baseline.score_pct,
                summary.baseline.elo_diff,
                summary.baseline.elo_error,
                summary.baseline.los * 100.0,
            );
            println!(
                "  H2H:       W{} D{} L{}  {:.2}%  Elo {:+.1} +/- {:.1}  LOS {:.1}%  {:?}",
                summary.head_to_head.result.wins,
                summary.head_to_head.result.draws,
                summary.head_to_head.result.losses,
                summary.head_to_head.score_pct,
                summary.head_to_head.result.elo_diff,
                summary.head_to_head.result.elo_error,
                summary.head_to_head.result.los * 100.0,
                summary.head_to_head.result.sprt_result,
            );
            println!(
                "  Delta:     {:+.2} pct (candidate vs baseline suite score)",
                summary.score_delta_pct
            );
            println!("  Verdict:   {:?}", summary.verdict);
            println!("  Output:    {}", output.display());
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

        Commands::Export { output } => {
            let storage = open_storage(&config)?;
            let bundle = build_export_bundle(&storage, &config)?;
            let payload = serde_json::to_vec_pretty(&bundle)?;
            let output = output.unwrap_or_else(default_export_path);
            std::fs::write(&output, payload)?;
            println!("Exported Crucible data to {}", output.display());
        }
    }

    Ok(())
}

fn default_export_path() -> PathBuf {
    PathBuf::from(format!(
        "crucible-export-{}.json",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    ))
}

/// The main continuous testing loop
async fn run_test_loop(storage: Storage, shared_config: SharedConfig) {
    info!("Test loop started");
    match storage.requeue_running_jobs() {
        Ok(0) => {}
        Ok(count) => warn!(
            "Re-queued {} interrupted running job(s) after restart",
            count
        ),
        Err(err) => tracing::error!("Failed to recover interrupted jobs: {}", err),
    }

    let mut workers = tokio::task::JoinSet::new();
    let mut last_poll_at = tokio::time::Instant::now();
    let mut first_poll = true;

    loop {
        let config = current_config(&shared_config);
        let worker_count = usize::try_from(config.testing.concurrency.max(1)).unwrap_or(1);
        let poll_interval = std::time::Duration::from_secs(config.testing.poll_interval_seconds);

        if first_poll || last_poll_at.elapsed() >= poll_interval {
            sync_and_schedule_engines(&storage, &config);
            last_poll_at = tokio::time::Instant::now();
            first_poll = false;
        }

        fill_worker_slots(&mut workers, &storage, &config, worker_count);

        let next_poll_at = last_poll_at + poll_interval;
        if workers.is_empty() {
            let sleep_for = std::cmp::min(
                next_poll_at.saturating_duration_since(tokio::time::Instant::now()),
                std::time::Duration::from_secs(1),
            );
            tokio::time::sleep(sleep_for).await;
            continue;
        }

        tokio::select! {
            result = workers.join_next() => {
                if let Some(result) = result {
                    if let Err(err) = result {
                        tracing::error!("Job worker task panicked: {}", err);
                    }
                }
            }
            _ = tokio::time::sleep_until(next_poll_at) => {}
        }
    }
}

fn sync_and_schedule_engines(storage: &Storage, config: &Config) {
    let engines = match storage.get_engines() {
        Ok(e) => e,
        Err(err) => {
            tracing::error!("Failed to get engines: {}", err);
            return;
        }
    };

    for engine in &engines {
        let git_mgr = GitManager::new(
            &engine.repo_url,
            &engine.local_path,
            &engine.build_cmd,
            &engine.binary_path,
        );

        match git_mgr.ensure_repo() {
            Ok(repo) => {
                if let Err(e) = sync_engine_revisions(storage, engine, &git_mgr, &repo) {
                    tracing::error!("Failed to sync revisions for '{}': {}", engine.name, e);
                }
            }
            Err(e) => {
                tracing::error!("Failed to access repo for '{}': {}", engine.name, e);
            }
        }

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
}

fn fill_worker_slots(
    workers: &mut tokio::task::JoinSet<()>,
    storage: &Storage,
    config: &Config,
    worker_count: usize,
) {
    while workers.len() < worker_count {
        match storage.claim_next_job() {
            Ok(Some(job)) => {
                let job_storage = storage.clone();
                let job_config = config.clone();
                workers.spawn(async move {
                    process_claimed_job(job_storage, job_config, job).await;
                });
            }
            Ok(None) => break,
            Err(err) => {
                tracing::error!("Failed to claim next job: {}", err);
                break;
            }
        }
    }

    while workers.len() < worker_count
        && config.training.idle_selfplay
        && !has_pending_test_jobs(storage)
    {
        let Some(task) = next_idle_selfplay_task(storage, config) else {
            break;
        };
        workers.spawn(async move {
            if let Err(err) = run_idle_selfplay_batch(task).await {
                tracing::error!("Idle self-play batch failed: {}", err);
            }
        });
    }
}

fn current_config(shared_config: &SharedConfig) -> Config {
    shared_config
        .read()
        .expect("shared config poisoned")
        .clone()
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
            experimental_branches: engine_cfg.experimental_branches.clone(),
            build_cmd: engine_cfg.build_cmd.clone(),
            binary_path: engine_cfg.binary_path.clone(),
            start_from: engine_cfg.start_from.clone(),
        };
        storage.insert_engine(&engine)?;
    }
    Ok(())
}

fn resolve_selfplay_revision(
    storage: &Storage,
    engine: &Engine,
    revision: Option<&str>,
) -> Result<crucible::types::EngineRevision> {
    if let Some(revision) = revision {
        return storage
            .get_revision_by_ref_prefix(&engine.id, revision)?
            .with_context(|| format!("Could not resolve revision '{}'", revision));
    }

    storage
        .get_revisions_for_engine(&engine.id)?
        .into_iter()
        .rev()
        .find(|revision| revision.build_status == BuildStatus::Success)
        .with_context(|| {
            format!(
                "Engine '{}' has no successfully built revisions",
                engine.name
            )
        })
}

fn resolve_engine_revision(
    storage: &Storage,
    engine: &Engine,
    revision: &str,
) -> Result<crucible::types::EngineRevision> {
    storage
        .get_revision_by_ref_prefix(&engine.id, revision)?
        .with_context(|| format!("Could not resolve revision '{}'", revision))
}

struct IdleSelfplayTask {
    engine: Engine,
    revision: crucible::types::EngineRevision,
    config: Config,
}

fn has_pending_test_jobs(storage: &Storage) -> bool {
    match storage.get_system_status() {
        Ok(status) => status.queued_jobs > 0,
        Err(_) => true,
    }
}

fn next_idle_selfplay_task(storage: &Storage, config: &Config) -> Option<IdleSelfplayTask> {
    let engines = storage.get_engines().ok()?;
    let engine = engines.into_iter().next()?;
    let revision = storage
        .get_revisions_for_engine(&engine.id)
        .ok()?
        .into_iter()
        .rev()
        .find(|revision| revision.build_status == BuildStatus::Success)?;
    Some(IdleSelfplayTask {
        engine,
        revision,
        config: config.clone(),
    })
}

async fn run_idle_selfplay_batch(task: IdleSelfplayTask) -> Result<()> {
    let binary_path = task.revision.binary_path.clone().with_context(|| {
        format!(
            "Revision '{}' does not have a built binary",
            task.revision.commit_hash
        )
    })?;

    let summary = run_selfplay_data_generation(SelfPlayDataConfig {
        engine_id: task.engine.id.clone(),
        engine_name: task.engine.name.clone(),
        revision_id: task.revision.id.clone(),
        revision_hash: task.revision.commit_hash.clone(),
        binary_path,
        time_control: configured_time_control(&task.config),
        opening_book: load_opening_book(task.config.testing.opening_book.as_deref())?,
        games: task.config.training.idle_batch_games,
        hash_mb: task.config.testing.hash_mb,
        threads: task.config.testing.engine_threads,
        output_dir: task.config.training.output_dir.clone(),
        depth: task.config.training.selfplay_depth,
        kind: TrainingRunKind::Idle,
    })?;

    info!(
        "Completed idle self-play batch for '{}' at {}: {} games, {} samples",
        task.engine.name,
        short_hash(&task.revision.commit_hash),
        summary.games_played,
        summary.samples_written
    );
    Ok(())
}

async fn process_claimed_job(storage: Storage, config: Config, job: TestJob) {
    info!(
        "Running job {} (dev={}, base={})",
        job.id, job.dev_revision_id, job.base_revision_id
    );

    match execute_job(&storage, &config, &job).await {
        Ok(result) => {
            if matches!(
                storage.get_job_status(&job.id),
                Ok(Some(TestStatus::Cancelled))
            ) {
                info!("Job {} was cancelled before results were persisted", job.id);
                let _ = storage.mark_bisect_session_failed_for_job(&job.id);
                return;
            }
            if let Err(err) = persist_job_result(&storage, &job, &result) {
                tracing::error!("Failed to persist job {}: {}", job.id, err);
                let _ = storage.set_job_status(&job.id, TestStatus::Failed);
                let _ = storage.mark_bisect_session_failed_for_job(&job.id);
                return;
            }
            info!(
                "Completed job {} after {} games: W{} D{} L{} ({:?})",
                job.id,
                result.total_games(),
                result.wins,
                result.draws,
                result.losses,
                result.sprt_result
            );

            if let Err(err) = advance_bisect_after_job(&storage, &config, &job, &result) {
                tracing::error!("Failed to advance bisect for job {}: {}", job.id, err);
                let _ = storage.mark_bisect_session_failed_for_job(&job.id);
            }
        }
        Err(err) => {
            if matches!(
                storage.get_job_status(&job.id),
                Ok(Some(TestStatus::Cancelled))
            ) {
                info!("Job {} cancelled", job.id);
                let _ = storage.mark_bisect_session_failed_for_job(&job.id);
                return;
            }
            tracing::error!("Job {} failed: {}", job.id, err);
            let _ = storage.set_job_status(&job.id, TestStatus::Failed);
            let _ = storage.mark_bisect_session_failed_for_job(&job.id);
        }
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
            min_games: config.testing.sprt.min_games,
        },
    }
}

async fn execute_job(storage: &Storage, config: &Config, job: &TestJob) -> Result<TestResult> {
    let engine = storage
        .get_engine_by_id(&job.engine_id)?
        .with_context(|| format!("Missing engine '{}'", job.engine_id))?;
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

    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_storage = storage.clone();
    let cancel_job_id = job.id.clone();
    let cancel_watch = {
        let cancel_flag = cancel_flag.clone();
        tokio::spawn(async move {
            loop {
                match cancel_storage.get_job_status(&cancel_job_id) {
                    Ok(Some(TestStatus::Cancelled)) | Ok(None) => {
                        cancel_flag.store(true, Ordering::Relaxed);
                        break;
                    }
                    Ok(Some(TestStatus::Completed | TestStatus::Failed)) => break,
                    Ok(Some(TestStatus::Queued | TestStatus::Running)) => {}
                    Err(_) => {}
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        })
    };
    let progress_storage = storage.clone();
    let progress_job_id = job.id.clone();
    let training_exports = if config.training.collect_from_tests {
        Some(build_regression_training_exports(
            config,
            &engine,
            &dev_revision,
            &base_revision,
            job,
        )?)
    } else {
        None
    };
    let progress_task = tokio::spawn(async move {
        persist_job_progress(
            progress_storage,
            progress_job_id,
            event_rx,
            training_exports,
        )
        .await
    });
    configured_sprt_bounds(config, job.job_type).validate()?;
    let result = run_match(
        MatchConfig {
            dev_binary,
            base_binary,
            dev_options: Vec::new(),
            base_options: Vec::new(),
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
            cancel_flag: Some(cancel_flag),
        },
        event_tx,
    )
    .await;
    cancel_watch.abort();
    let mut training_exports = match progress_task.await {
        Ok(Ok(exports)) => exports,
        Ok(Err(err)) => return Err(err),
        Err(err) => return Err(anyhow!("job progress task panicked: {}", err)),
    };
    match &result {
        Ok(_) => {}
        Err(err) if err.to_string().contains("match cancelled") => {
            if let Some(exports) = training_exports.as_mut() {
                exports.set_status(TrainingRunStatus::Cancelled)?;
            }
        }
        Err(_) => {
            if let Some(exports) = training_exports.as_mut() {
                exports.set_status(TrainingRunStatus::Failed)?;
            }
        }
    };
    result
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

    storage.set_job_status(&job.id, TestStatus::Completed)?;
    Ok(())
}

async fn persist_job_progress(
    storage: Storage,
    job_id: String,
    mut event_rx: mpsc::UnboundedReceiver<MatchEvent>,
    mut training_exports: Option<RegressionTrainingExports>,
) -> Result<Option<RegressionTrainingExports>> {
    while let Some(event) = event_rx.recv().await {
        match event {
            MatchEvent::GameCompleted {
                record,
                training_samples,
                ..
            } => {
                storage.insert_game(&job_id, &record)?;
                if let Some(exports) = training_exports.as_mut() {
                    exports.record_game(record.game_number)?;
                    exports.append_samples(training_samples)?;
                }
            }
            MatchEvent::SprtUpdate {
                wins,
                draws,
                losses,
                llr_status,
            } => {
                storage.update_job_result(
                    &job_id,
                    wins,
                    losses,
                    draws,
                    wdl_to_elo(wins, draws, losses),
                    elo_error(wins, draws, losses),
                    los(wins, losses),
                    llr_status,
                )?;
            }
            MatchEvent::MatchCompleted { .. } => {
                if let Some(exports) = training_exports.as_mut() {
                    exports.set_status(TrainingRunStatus::Completed)?;
                }
            }
            MatchEvent::GameStarted { .. } | MatchEvent::Error { .. } => {}
        }
    }

    Ok(training_exports)
}

struct RegressionTrainingExports {
    dev: TrainingRunWriter,
    base: TrainingRunWriter,
}

impl RegressionTrainingExports {
    fn record_game(&mut self, game_number: u32) -> Result<()> {
        self.dev.record_game(game_number)?;
        self.base.record_game(game_number)?;
        Ok(())
    }

    fn append_samples(&mut self, samples: Vec<TaggedTrainingSample>) -> Result<()> {
        let mut dev_samples = Vec::new();
        let mut base_samples = Vec::new();

        for sample in samples {
            match sample.source {
                TrainingSampleSource::Dev => dev_samples.push(sample.sample),
                TrainingSampleSource::Base => base_samples.push(sample.sample),
            }
        }

        self.dev.append_samples(&dev_samples)?;
        self.base.append_samples(&base_samples)?;
        Ok(())
    }

    fn set_status(&mut self, status: TrainingRunStatus) -> Result<()> {
        self.dev.set_status(status)?;
        self.base.set_status(status)?;
        Ok(())
    }
}

fn build_regression_training_exports(
    config: &Config,
    engine: &Engine,
    dev_revision: &crucible::types::EngineRevision,
    base_revision: &crucible::types::EngineRevision,
    job: &TestJob,
) -> Result<RegressionTrainingExports> {
    let time_control = job.time_control.to_string();
    let dev = TrainingRunWriter::begin(
        &config.training.output_dir,
        TrainingRunDescriptor {
            engine_id: engine.id.clone(),
            engine_name: engine.name.clone(),
            revision_id: dev_revision.id.clone(),
            revision_hash: dev_revision.commit_hash.clone(),
            time_control: time_control.clone(),
            games_requested: Some(config.testing.max_games),
            kind: TrainingRunKind::Regression,
            source_job_id: Some(job.id.clone()),
            source_role: Some("dev".to_string()),
            depth_mode: crucible::training::TrainingDepthMode::Min,
            depth_value: config.training.regression_min_depth,
        },
    )?;
    let base = TrainingRunWriter::begin(
        &config.training.output_dir,
        TrainingRunDescriptor {
            engine_id: engine.id.clone(),
            engine_name: engine.name.clone(),
            revision_id: base_revision.id.clone(),
            revision_hash: base_revision.commit_hash.clone(),
            time_control,
            games_requested: Some(config.testing.max_games),
            kind: TrainingRunKind::Regression,
            source_job_id: Some(job.id.clone()),
            source_role: Some("base".to_string()),
            depth_mode: crucible::training::TrainingDepthMode::Min,
            depth_value: config.training.regression_min_depth,
        },
    )?;
    Ok(RegressionTrainingExports { dev, base })
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
