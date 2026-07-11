---
title: Configuration reference
nav_order: 6
---

# Configuration reference

Crucible reads its configuration from `crucible.toml` in the working directory. You can pass `--config <path>` to any command to use a different file. Running `crucible init` writes a complete example you can edit in place.

This page documents every field. Defaults are shown in parentheses.

## A complete example

```toml
[server]
web_port = 8877
web_host = "127.0.0.1"    # use "0.0.0.0" in Docker
# Required when web_host is not loopback. Generate with: openssl rand -hex 32
# admin_token = "paste-generated-token-here"

[testing]
concurrency = 4
max_games = 10000
progression_games = 200
hash_mb = 16
engine_threads = 1
poll_interval_seconds = 60

[testing.time_control]
base_ms = 10000
increment_ms = 100

[testing.sprt]
elo0 = 0.0
elo1 = 5.0
alpha = 0.05
beta = 0.05
min_games = 16

[training]
output_dir = ".crucible/training"
selfplay_games = 100
collect_from_tests = true
selfplay_depth = 10
regression_min_depth = 10
idle_selfplay = false
idle_batch_games = 1

[[gate.opponents]]
name = "Stockfish"
binary_path = "/opt/engines/stockfish"

[[gate.opponents]]
name = "Ethereal"
binary_path = "/opt/engines/ethereal"

[[gate.profiles]]
name = "release"
opponents = ["Stockfish", "Ethereal"]
games_per_opponent = 100
min_score_delta = 0.0

[[engines]]
name = "my-engine"
repo = "https://github.com/you/your-engine"
branches = ["main", "release/*"]
experimental_branches = ["exp/*"]
build_cmd = "make"
binary_path = "my-engine"
start_from = "v1.0.0"
```

## Top-level

| Field      | Default       | Notes                                                                     |
| ---------- | ------------- | ------------------------------------------------------------------------- |
| `data_dir` | `./.crucible` | Where the SQLite database, cloned repositories, and build artifacts live. |

## `[server]`

| Field         | Default     | Notes                                                                                                           |
| ------------- | ----------- | --------------------------------------------------------------------------------------------------------------- |
| `web_host`    | `127.0.0.1` | Interface the dashboard binds to. Use `0.0.0.0` when running in Docker.                                         |
| `web_port`    | `8877`      | Port the dashboard listens on.                                                                                  |
| `admin_token` | unset       | Required when `web_host` is not loopback. The dashboard sends it as a `Bearer` token on protected API requests. |

Crucible refuses to start with common placeholder tokens such as `changeme`, `change-me`, or `paste-generated-token-here`. The browser client stores the admin token in local storage. Clear the tab data, or use a fresh browser profile, if you want to reset it.

## `[testing]`

| Field                   | Default | Notes                                                                                                      |
| ----------------------- | ------- | ---------------------------------------------------------------------------------------------------------- |
| `concurrency`           | `1`     | Number of test jobs that run in parallel.                                                                  |
| `max_games`             | `10000` | Maximum games per SPRT match before the test gives up as inconclusive.                                     |
| `progression_games`     | unset   | Fixed game count for canonical sequential history. Must be even; when unset, these matches also use SPRT.  |
| `hash_mb`               | `16`    | Hash table size passed to every engine instance (`setoption name Hash value ...`).                         |
| `engine_threads`        | `1`     | Threads per engine instance (`setoption name Threads value ...`).                                          |
| `poll_interval_seconds` | `60`    | How often the daemon fetches new commits and schedules fresh jobs.                                         |
| `opening_book`          | unset   | Path to an EPD opening suite containing one position per line (see below).                                 |

Set `progression_games` when the Timeline is primarily an Elo-estimation view. Canonical commit-to-commit matches then play the same number of color-balanced games and finish with a `FixedGames` result. Experimental branches, manual patch tests, and regression hunts continue to use SPRT, where an early hypothesis decision is the useful outcome.

The opening book is a plain text EPD file. Each non-empty, non-comment line must contain an EPD position: the first four FEN fields, followed by optional semicolon-terminated EPD operations. Lines beginning with `#` are ignored. Crucible honors `hmvc` and `fmvn` operations when present, defaulting them to `0` and `1` otherwise. Other EPD operations such as `bm` and `id` are accepted as metadata but are not used by the match runner.

```text
# openings/stc.epd
rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq -
rnbqkbnr/ppp2ppp/4p3/3p4/3PP3/8/PPP2PPP/RNBQKBNR w KQkq d6 hmvc 0; fmvn 3; id "QGD";
```

Use an explicit starting-position EPD instead of the old `startpos` literal. Full six-field FEN lines are not valid opening-book entries.

### `[testing.time_control]`

| Field          | Default | Notes                                                                     |
| -------------- | ------- | ------------------------------------------------------------------------- |
| `base_ms`      | `10000` | Base time in milliseconds (10 seconds by default).                        |
| `increment_ms` | `100`   | Increment per move in milliseconds.                                       |
| `nodes`        | unset   | If set, the match uses a fixed nodes-per-move limit instead of the clock. |

### `[testing.sprt]`

| Field       | Default | Notes                                                                                        |
| ----------- | ------- | -------------------------------------------------------------------------------------------- |
| `elo0`      | `0.0`   | Null hypothesis: "no improvement".                                                           |
| `elo1`      | `5.0`   | Alternative hypothesis: "at least this many Elo points gained".                              |
| `alpha`     | `0.05`  | False positive rate.                                                                         |
| `beta`      | `0.05`  | False negative rate.                                                                         |
| `min_games` | `16`    | Minimum games that must complete before SPRT is allowed to stop, so tiny samples do not win. |

Regression-hunt probes always use fixed bounds tuned for detecting a drop, regardless of this block.

## `[training]`

| Field                  | Default              | Notes                                                                                           |
| ---------------------- | -------------------- | ----------------------------------------------------------------------------------------------- |
| `output_dir`           | `.crucible/training` | Root directory for exported training data.                                                      |
| `selfplay_games`       | `100`                | Default games per `crucible selfplay-data` run.                                                 |
| `selfplay_depth`       | `10`                 | Exact reported search depth kept in a self-play export.                                         |
| `collect_from_tests`   | `true`               | When true, regression matches also write training JSONL alongside the usual results.            |
| `regression_min_depth` | `10`                 | Minimum reported depth kept when collecting from regression matches.                            |
| `idle_selfplay`        | `false`              | When true, idle worker slots generate short self-play batches whenever the test queue is empty. |
| `idle_batch_games`     | `1`                  | Games per idle self-play batch.                                                                 |

See [Training data](training-data.md) for the full layout of exported files.

## `[[gate.opponents]]`

Each `[[gate.opponents]]` entry names an external engine binary that gate profiles can reference.

| Field         | Default | Notes                                                                           |
| ------------- | ------- | ------------------------------------------------------------------------------- |
| `name`        | -       | Short name referenced by `gate.profiles.opponents`.                             |
| `binary_path` | -       | Absolute path to the opponent's UCI binary.                                     |
| `options`     | `{}`    | Map of UCI option names to string values set on the opponent before each match. |

## `[[gate.profiles]]`

Each `[[gate.profiles]]` entry defines a named gauntlet.

| Field                | Default | Notes                                                                                    |
| -------------------- | ------- | ---------------------------------------------------------------------------------------- |
| `name`               | -       | Short name passed to `crucible gate --profile`.                                          |
| `opponents`          | -       | List of opponent names defined in `[[gate.opponents]]`.                                  |
| `games_per_opponent` | `100`   | Even game count played against each opponent by both candidate and baseline.              |
| `min_score_delta`    | `0.0`   | Minimum score-percentage delta, candidate minus baseline, required for a `Pass` verdict. |
| `opening_book`       | unset   | Optional per-profile opening book, overrides `testing.opening_book` during the gate.     |
| `time_control`       | unset   | Optional per-profile time control, overrides `testing.time_control` during the gate.     |

See [Release gates](release-gates.md) for how the verdict is computed.

## `[[engines]]`

Each `[[engines]]` entry describes a tracked engine. Entries are imported into the database when the daemon starts, and any engine added via `crucible add` is also stored there.

| Field                   | Default    | Notes                                                                                                                |
| ----------------------- | ---------- | -------------------------------------------------------------------------------------------------------------------- |
| `name`                  | -          | Human-readable engine name, used throughout the dashboard and CLI.                                                   |
| `repo`                  | -          | Remote Git URL that Crucible clones into `data_dir/repos/<name>`.                                                    |
| `branches`              | `["main"]` | Branches to track. Entries can be exact names or wildcards (for example `release/*`) matched against `origin/*`.     |
| `experimental_branches` | `[]`       | Branches tested normally but kept out of the canonical Timeline and Jobs views. They show up in the Experiments tab. |
| `build_cmd`             | -          | Shell command that produces the engine binary. Runs inside the cloned repository.                                    |
| `binary_path`           | -          | Path to the built binary, relative to the repository root. It cannot be absolute or contain `.`/`..` components.     |
| `start_from`            | unset      | Skip all commits before this ref. Useful for engines with a long pre-Crucible history.                               |

If `branches` and `experimental_branches` are both empty, Crucible refuses to start and prints an error pointing at the offending engine.

Engine names are also used as directory names under `data_dir/repos`, so they must be a single path component and cannot contain `/`, `\`, or be `.`/`..`.

## Engine runtimes

Crucible can test any UCI engine that can be built and executed on the host or inside the container. The published Docker image includes common Rust, Go, C/C++, Zig, .NET/C#, Java/Maven, JavaScript/npm, and Python/pip tools. Zig is pinned to `0.15.2`. Haskell, unusual SDK versions, host-specific dependencies, and other version-sensitive ecosystems are often simpler with a local binary; Docker can still work with a custom image or mounted toolchain. See [Docker](docker.md) and [Engine runtimes](engine-runtimes.md) for examples.

A typical entry for a Zig engine looks like this:

```toml
[[engines]]
name = "sykora"
repo = "https://github.com/sb2bg/sykora"
branches = ["main"]
build_cmd = "zig build -Doptimize=ReleaseFast"
binary_path = "zig-out/bin/sykora"
start_from = "v0.2.1"
```
