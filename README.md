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
- **Self-Play Data Export** — Generate NNUE-style JSONL data from self-play and bucket it by reported search depth under engine/revision-specific directories.
- **Zero Dependencies** — Single binary, SQLite storage, embedded web UI. No Docker, no Django, no external services.

## Quick Start

```bash
# Build from source
cargo build --release

# Initialize config
./target/release/crucible init

# Add your engine
./target/release/crucible add \
  --name my-engine \
  --repo https://github.com/you/your-engine \
  --build "make" \
  --binary-path "target/release/my-engine" \
  --branches main,dev \
  --start-from v1.0.0

# Start testing
./target/release/crucible run

# Or with the terminal monitor
./target/release/crucible run --tui
```

The web dashboard is available at `http://localhost:8877` by default.

The dashboard now includes an admin tab for adding/removing engines, queueing manual tests, starting regression hunts, cancelling queued/running jobs, and downloading a JSON export bundle, plus a training tab that summarizes self-play export runs and depth-bucket counts. If you expose it beyond localhost, put it behind an auth layer such as Cloudflare Access, Tailscale, or a reverse proxy with access control.

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
web_host = "127.0.0.1"    # use "0.0.0.0" in Docker
admin_token = "change-me" # optional; protects /api/admin/* with Bearer auth

[testing]
concurrency = 4            # Test jobs to run in parallel
max_games = 10000          # Max games per test before giving up
hash_mb = 16               # Hash table size for engines
engine_threads = 1         # Threads per engine instance
poll_interval_seconds = 60 # how often the daemon checks for new commits/jobs

[training]
output_dir = ".crucible/training"
selfplay_games = 100
collect_from_tests = true
selfplay_depth = 10
regression_min_depth = 10
idle_selfplay = false
idle_batch_games = 1

[testing.time_control]
base_ms = 10000            # 10+0.1 STC
increment_ms = 100

[testing.sprt]
elo0 = 0.0                 # H0: no improvement
elo1 = 5.0                 # H1: 5 Elo improvement
alpha = 0.05               # False positive rate
beta = 0.05                # False negative rate
min_games = 16             # don't let SPRT conclude on tiny samples

[[engines]]
name = "my-engine"
repo = "https://github.com/you/your-engine"
branches = ["main", "release/*"]
experimental_branches = ["exp/*"]
build_cmd = "make"
binary_path = "my-engine"
start_from = "v1.0.0"
```

Entries under `[[engines]]` are imported automatically when `crucible run` starts.
`branches` entries can be exact names or wildcard patterns like `exp/*`, matched against remote `origin/...` branches.
`experimental_branches` are tested normally, but the default Timeline and Jobs views keep them out of the canonical history and show them in the separate Experiments tab.
For Zig-based engines, just use a Zig `build_cmd`. The Docker image ships with Zig preinstalled.
If `server.admin_token` is set, the web admin panel sends it as a Bearer token; the browser stores it locally until you clear it.

`testing.opening_book` currently expects a plain text file with one opening per line. Each line must be either `startpos` or a full FEN. Comment lines starting with `#` are ignored. PGN/EPD parsing is not implemented yet.

## Training Data

You can generate self-play data for a specific engine revision:

```bash
crucible selfplay-data --engine Sykora --games 200
```

By default, Crucible uses the latest successfully built revision for that engine, reuses your configured time control, and writes JSONL under:

```text
.crucible/training/<engine>/<revision>/<run-timestamp>/
```

Each run is split into files like `depth-010.jsonl`, `depth-011.jsonl`, and so on. Dedicated self-play runs keep only positions reported at exactly `training.selfplay_depth`, so `selfplay_depth = 10` gives you D10 data. Regression/SPRT collection is separate: it keeps positions at or above `training.regression_min_depth`, so you can still harvest D10+ data from test matches. Every row includes engine and revision metadata, the FEN, side to move, reported depth, score, chosen move, and final game result from that side's perspective.

To pin a specific revision or change the destination:

```bash
crucible selfplay-data \
  --engine Sykora \
  --revision eb640fa6 \
  --games 500 \
  --depth 12 \
  --output-dir /data/nnue
```

When `training.idle_selfplay = true`, Crucible uses spare worker slots for short self-play batches whenever there are no queued test jobs. That lets the server keep generating training data in the background instead of idling.

## Exporting Results

You can export the current Crucible state as a single JSON bundle for external analysis or LLM ingestion:

```bash
crucible export
```

By default this writes a timestamped file like `crucible-export-20260405T120000Z.json` in the current directory. You can choose a path explicitly:

```bash
crucible export --output /tmp/crucible.json
```

The admin tab also has a `Download export` button that returns the same JSON bundle over the web UI.

## CI/CD

GitHub Actions now includes a workflow that:

- runs `cargo fmt --check`, `cargo check --locked`, and `cargo test --locked`
- builds the Docker image on pull requests
- publishes the image to GHCR on pushes to `main` and version tags

Published images go to `ghcr.io/sb2bg/crucible`.

## Commands

| Command                                                   | Description                       |
| --------------------------------------------------------- | --------------------------------- |
| `crucible init`                                           | Generate example `crucible.toml`  |
| `crucible run`                                            | Start continuous testing + web UI |
| `crucible run --tui`                                      | Start with terminal monitor       |
| `crucible monitor`                                        | Attach TUI to running instance    |
| `crucible add ...`                                        | Add an engine to track            |
| `crucible list`                                           | List tracked engines              |
| `crucible remove --name <n> [--delete-data]`              | Remove a tracked engine           |
| `crucible status`                                         | Show current testing status       |
| `crucible export [--output <path>]`                       | Export results as JSON            |
| `crucible bisect --engine <n> --good <hash> --bad <hash>` | Start a regression hunt           |
| `crucible test --engine <n> --dev <hash> --base <hash>`   | Manual one-off test               |
| `crucible selfplay-data --engine <n> [--revision <hash>] [--depth <n>]` | Export self-play training data    |

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
