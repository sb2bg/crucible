use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crucible::config::Config;
use crucible::storage::Storage;

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

            // Ensure data directory
            std::fs::create_dir_all(&config.data_dir)?;
            let db_path = config.data_dir.join("crucible.db");
            let storage = Storage::open(&db_path)?;

            // Start web server
            let web_storage = storage.clone();
            let web_host = config.server.web_host.clone();
            let web_port = config.server.web_port;

            let web_handle = tokio::spawn(async move {
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
            let db_path = config.data_dir.join("crucible.db");
            let storage = Storage::open(&db_path)?;
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
            std::fs::create_dir_all(&config.data_dir)?;
            let db_path = config.data_dir.join("crucible.db");
            let storage = Storage::open(&db_path)?;

            let engine = crucible::types::Engine {
                id: uuid::Uuid::new_v4().to_string(),
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
            let db_path = config.data_dir.join("crucible.db");
            if !db_path.exists() {
                println!("No engines tracked. Run `crucible add` first.");
                return Ok(());
            }
            let storage = Storage::open(&db_path)?;
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
            let db_path = config.data_dir.join("crucible.db");
            let storage = Storage::open(&db_path)?;

            println!(
                "Starting bisect for '{}': good={}, bad={}",
                engine, good, bad
            );
            println!("This will binary-search for the commit that caused the regression.");
            println!("Jobs will be queued with highest priority.");
            // TODO: Wire up bisect runner
        }

        Commands::Test { engine, dev, base } => {
            let db_path = config.data_dir.join("crucible.db");
            let storage = Storage::open(&db_path)?;

            println!(
                "Queuing manual test: {} vs {} for '{}'",
                &dev[..8.min(dev.len())],
                &base[..8.min(base.len())],
                engine
            );
            // TODO: Wire up manual test scheduling
        }

        Commands::Status { engine } => {
            let db_path = config.data_dir.join("crucible.db");
            if !db_path.exists() {
                println!("No data yet. Run `crucible run` first.");
                return Ok(());
            }
            let storage = Storage::open(&db_path)?;
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
            let git_mgr = crucible::git::GitManager::new(
                &engine.repo_url,
                &engine.local_path,
                &engine.build_cmd,
                &engine.binary_path,
            );

            match git_mgr.ensure_repo() {
                Ok(repo) => {
                    for branch in &engine.branches {
                        match git_mgr.list_commits(
                            &repo,
                            branch,
                            &engine.id,
                            engine.start_from.as_deref(),
                        ) {
                            Ok(revisions) => {
                                info!(
                                    "Engine '{}' branch '{}': {} commits",
                                    engine.name,
                                    branch,
                                    revisions.len()
                                );
                                for rev in &revisions {
                                    let _ = storage.insert_revision(rev);
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to list commits for {}/{}: {}",
                                    engine.name,
                                    branch,
                                    e
                                );
                            }
                        }
                    }

                    // Build pending revisions
                    if let Ok(revisions) = storage.get_revisions_for_engine(&engine.id) {
                        for rev in revisions
                            .iter()
                            .filter(|r| r.build_status == crucible::types::BuildStatus::Pending)
                        {
                            match git_mgr.build_revision(&repo, &rev.commit_hash) {
                                Ok(binary) => {
                                    let _ = storage.update_build_status(
                                        &rev.id,
                                        crucible::types::BuildStatus::Success,
                                        Some(&binary),
                                    );
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Build failed for {}: {}",
                                        &rev.commit_hash[..8],
                                        e
                                    );
                                    let _ = storage.update_build_status(
                                        &rev.id,
                                        crucible::types::BuildStatus::Failed,
                                        None,
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to access repo for '{}': {}", engine.name, e);
                }
            }

            // Schedule and run test jobs
            let scheduler = crucible::scheduler::Scheduler::new(storage.clone(), config.clone());
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
        while let Ok(Some(job)) = storage.get_next_job() {
            info!(
                "Running job {} (dev={}, base={})",
                job.id, job.dev_revision_id, job.base_revision_id
            );
            let _ = storage.set_job_status(&job.id, crucible::types::TestStatus::Running);

            // Get binary paths from revisions
            // TODO: Actually run the match here using engine::match_runner
            // For now, mark as completed to prevent infinite loop
            let _ = storage.set_job_status(&job.id, crucible::types::TestStatus::Completed);
        }

        // Sleep before next polling cycle
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}
