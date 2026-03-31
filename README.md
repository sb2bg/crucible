# ⚗ Crucible

[![Docker](https://github.com/sb2bg/crucible/actions/workflows/docker-publish.yml/badge.svg)](https://github.com/sb2bg/crucible/actions/workflows/docker-publish.yml)

**CI for chess engines** — automated SPRT regression testing across your git history.

Crucible continuously builds and tests every commit of your chess engine, showing you an Elo timeline so you always know which changes made your engine stronger or weaker. Think of it as what CI/CD is for software correctness, but for chess engine _strength_.

## Why?

Existing tools like [OpenBench](https://github.com/AndyGrant/OpenBench) are designed for distributed testing across teams. They're great for Stockfish-scale projects, but overkill for a solo engine developer who just wants to know: **"Did my last 5 commits make things better or worse?"**

Crucible is built for the solo dev. One command, your machine, your engine, your answers.

## Features

- **Elo Timeline** — The main view. See your engine's estimated Elo across every commit, with confidence intervals. Tags and releases are highlighted.
- **Automatic SPRT** — Each commit is tested against the previous one using the Sequential Probability Ratio Test. Testing stops as soon as statistical significance is reached.
- **Regression Hunt** — Notice a regression? Point Crucible at a good commit and a bad commit, and it samples the range against a fixed baseline, narrows to the first bad window, and confirms the likely culprit.
- **Smart Scheduling** — Branch HEADs and tagged releases are tested first. Older commits backfill in the background. New pushes jump the queue.
- **Multi-Engine, Multi-Branch** — Track multiple engines and branches simultaneously.
- **Dual UI** — Terminal (TUI) for quick monitoring, web dashboard for deep dives and charts.
- **PGN Archive** — Every game is saved and browsable.
- **Zero Dependencies** — Single binary, SQLite storage, embedded web UI. No Docker, no Django, no external services.

## Quick Start

```bash
# Install
cargo install crucible

# Initialize config
crucible init

# Add your engine
crucible add \
  --name my-engine \
  --repo https://github.com/you/your-engine \
  --build "make" \
  --binary-path "target/release/my-engine" \
  --branches main,dev \
  --start-from v1.0.0

# Start testing
crucible run

# Or with the terminal monitor
crucible run --tui
```

The web dashboard is available at `http://localhost:8877` by default.

## Docker

For containerized deployments, set `web_host = "0.0.0.0"` so the dashboard is reachable outside the container.

```toml
[server]
web_host = "0.0.0.0"
web_port = 8877
```

Then start Crucible with Docker Compose:

```bash
cargo run -- init
docker compose up --build -d
```

The provided `docker-compose.yml` mounts:
- `./crucible.toml` into `/work/crucible.toml`
- a named Docker volume at `/work/.crucible` for the SQLite DB, cloned repos, and build artifacts

If you prefer a direct `docker run`, use:

```bash
docker build -t crucible .
docker run -d \
  --name crucible \
  -p 8877:8877 \
  -v "$(pwd)/crucible.toml:/work/crucible.toml:ro" \
  -v crucible-data:/work/.crucible \
  crucible
```

## Configuration

Edit `crucible.toml`:

```toml
[server]
web_port = 8877
web_host = "127.0.0.1"   # use "0.0.0.0" in Docker

[testing]
concurrency = 4           # Test jobs to run in parallel
max_games = 10000          # Max games per test before giving up
hash_mb = 16               # Hash table size for engines
engine_threads = 1         # Threads per engine instance

[testing.time_control]
base_ms = 10000            # 10+0.1 STC
increment_ms = 100

[testing.sprt]
elo0 = 0.0                 # H0: no improvement
elo1 = 5.0                 # H1: 5 Elo improvement
alpha = 0.05               # False positive rate
beta = 0.05                # False negative rate

[[engines]]
name = "my-engine"
repo = "https://github.com/you/your-engine"
branches = ["main", "dev"]
build_cmd = "make"
binary_path = "my-engine"
start_from = "v1.0.0"
```

Entries under `[[engines]]` are imported automatically when `crucible run` starts.

## CI/CD

GitHub Actions now includes a workflow that:
- runs `cargo fmt --check`, `cargo check --locked`, and `cargo test --locked`
- builds the Docker image on pull requests
- publishes the image to GHCR on pushes to `main` and version tags

Published images go to `ghcr.io/<your-github-username>/crucible`.

## Commands

| Command                                                   | Description                       |
| --------------------------------------------------------- | --------------------------------- |
| `crucible init`                                           | Generate example `crucible.toml`  |
| `crucible run`                                            | Start continuous testing + web UI |
| `crucible run --tui`                                      | Start with terminal monitor       |
| `crucible monitor`                                        | Attach TUI to running instance    |
| `crucible add ...`                                        | Add an engine to track            |
| `crucible list`                                           | List tracked engines              |
| `crucible status`                                         | Show current testing status       |
| `crucible bisect --engine <n> --good <hash> --bad <hash>` | Start a regression hunt           |
| `crucible test --engine <n> --dev <hash> --base <hash>`   | Manual one-off test               |

## How It Works

1. **Fetch** — Crucible clones your repo and enumerates commits on tracked branches
2. **Build** — Each commit is checked out and built using your build command
3. **Schedule** — The smart scheduler decides which commits to test next
4. **Test** — Pairs of engines play matches using the UCI protocol
5. **Analyze** — SPRT determines if the change is significant; Elo is estimated
6. **Repeat** — Crucible never stops. New commits are picked up automatically.

## Architecture

```
crucible (single Rust binary)
├── Git manager    — clone, fetch, enumerate commits, build
├── UCI engine     — communicate with chess engines
├── Match runner   — play games, handle time control
├── SPRT engine    — statistical testing
├── Scheduler      — smart job prioritization
├── Hunt runner    — sampled regression hunt + confirmation
├── Storage        — SQLite for all persistence
├── TUI            — ratatui terminal dashboard
└── Web server     — axum + embedded HTML dashboard
```

## License

GPL-3.0
