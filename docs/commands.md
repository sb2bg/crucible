---
title: CLI commands
nav_order: 4
---

# CLI commands

Every Crucible subcommand accepts `--config <path>` to point at a specific `crucible.toml`. The default is `crucible.toml` in the current directory.

## `crucible init`

Writes an example `crucible.toml` to the current directory. Safe to run in an empty project as a starting point.

## `crucible run`

Starts the daemon. The daemon does three things at once:

1. Polls each tracked engine's repository for new commits, at `testing.poll_interval_seconds`.
2. Serves the web dashboard on `server.web_host:server.web_port`.
3. Runs up to `testing.concurrency` test jobs in parallel, claiming work from the scheduler.

Pass `--tui` to launch the terminal UI alongside the daemon.

## `crucible monitor`

Attaches the terminal UI to an already running daemon. Both views read from the same SQLite database, so you can open and close the TUI without disrupting tests in progress.

## `crucible add`

Registers a new engine. All flags are required except `--experimental-branches` and `--start-from`.

```bash
crucible add \
  --name my-engine \
  --repo https://github.com/you/your-engine \
  --build "cargo build --release" \
  --binary-path "target/release/my-engine" \
  --branches main,dev \
  --experimental-branches exp/* \
  --start-from v1.0.0
```

`--branches` and `--experimental-branches` accept comma-separated lists. Wildcards are matched against `origin/...` refs, so `exp/*` picks up every branch with that prefix.

## `crucible list`

Prints a table of tracked engines, their remote URLs, and the branches each one follows.

## `crucible remove`

Removes a tracked engine from the database.

```bash
crucible remove --name my-engine
crucible remove --name my-engine --delete-data
```

Without `--delete-data`, the cloned repository and build artifacts under `data_dir/repos/<name>` are left on disk. With the flag, Crucible deletes the directory as well.

If the engine is also listed under `[[engines]]` in `crucible.toml`, it will be re-imported on the next `crucible run`. Remove it from the config file too if you want the removal to persist.

## `crucible status`

Prints a short summary of the current daemon state: engines tracked, running jobs, queued jobs, completed jobs, and total games played. You can pass an engine name as a positional argument, although at present the summary is global.

## `crucible test`

Queues a single SPRT match between two commits. Both hashes are resolved against the engine's revision table, so prefixes work.

```bash
crucible test \
  --engine my-engine \
  --dev a1b2c3d \
  --base v1.0.0
```

The match uses the engines' regular build artifacts. If either commit has not been built yet, the scheduler will build it when the job is claimed.

## `crucible bisect`

Starts a regression hunt between a known-good and known-bad commit.

```bash
crucible bisect \
  --engine my-engine \
  --good v1.0.0 \
  --bad HEAD
```

The hunt samples commits across the range against a fixed baseline, narrows to the first window in which the engine clearly regresses, and then confirms the likely culprit. See [Regression hunts](regression-hunts.md) for the full behaviour.

## `crucible gate`

Runs a release gate: candidate versus baseline over the same configured gauntlet of external engines.

```bash
crucible gate \
  --engine my-engine \
  --candidate exp/iir-off \
  --baseline v1.0.0 \
  --profile release
```

`--output` chooses where the JSON summary is written. By default the file lands under `data_dir/gates/<timestamp>-<profile>.json`. See [Release gates](release-gates.md) for the verdict rules and summary layout.

## `crucible selfplay-data`

Generates NNUE-style training data from self-play matches of a single engine revision.

```bash
crucible selfplay-data --engine my-engine --games 200
```

By default the command picks the latest successfully built revision for the engine, reuses the configured time control, and writes JSONL files under `training.output_dir`. Pin a revision, redirect the output, or override the depth by passing the relevant flag:

```bash
crucible selfplay-data \
  --engine my-engine \
  --revision a1b2c3d \
  --games 500 \
  --depth 12 \
  --output-dir /data/nnue
```

See [Training data](training-data.md) for the file layout and the schema of each sample.

## `crucible export`

Writes the current Crucible state to a single JSON bundle for external analysis or LLM ingestion.

```bash
crucible export
crucible export --output /tmp/crucible.json
```

Without `--output`, the command writes a timestamped file such as `crucible-export-20260405T120000Z.json` in the current directory. The admin tab on the dashboard offers the same export as a download.
