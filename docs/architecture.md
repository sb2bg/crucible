---
title: Architecture
nav_order: 11
---

# Architecture

Crucible is a single Rust binary. Everything it needs, including the dashboard, lives in the same process. Data is persisted to SQLite in the configured `data_dir`, so the daemon can be stopped and restarted without losing state.

## Pieces of the binary

| Module          | Responsibility                                                                    |
| --------------- | --------------------------------------------------------------------------------- |
| Git manager     | Clones tracked repositories, enumerates commits on matching branches, and builds each revision on demand using the engine's configured build command. |
| UCI adapter     | Talks to engine binaries over the standard UCI protocol.                          |
| Match runner    | Plays games, enforces time control, and streams events for SPRT bookkeeping and training-data collection. |
| SPRT            | Computes the log-likelihood ratio after every game, applies the configured bounds, and reports `H0Accepted`, `H1Accepted`, or `Inconclusive`. |
| Scheduler       | Prioritises work across engines and branches. See [Scheduling](scheduling.md).     |
| Bisect runner   | Drives regression hunts across a range of commits. See [Regression hunts](regression-hunts.md). |
| Gate runner     | Runs release gauntlets against external engine binaries. See [Release gates](release-gates.md). |
| Training writer | Writes JSONL training exports from self-play and regression matches.              |
| Storage         | SQLite schema, queries, migrations, and the claim-and-recover logic that keeps workers consistent across restarts. |
| Web server      | Axum router serving the JSON API and the embedded HTML dashboard.                  |
| TUI             | Ratatui-based terminal UI, used either alongside the daemon or attached later.     |

## The testing loop

1. **Fetch.** The scheduler polls every tracked engine's remote for new commits matching its configured branches.
2. **Build.** When a new commit is picked up, a worker checks it out and runs the engine's build command in the cloned repo.
3. **Schedule.** The scheduler decides which pair of revisions to test next, using the priorities described in [Scheduling](scheduling.md).
4. **Test.** A worker plays a match between the two revisions, streaming games into SQLite as they finish.
5. **Analyse.** SPRT is evaluated after every game. When it accepts a hypothesis, or the match hits `max_games`, the worker records the Elo estimate, error, and LOS, and releases the slot.
6. **Repeat.** The daemon never stops. New commits are picked up on the next poll, and the Timeline rolls forward.

## Storage layout

Everything Crucible persists lives under `data_dir`:

```
<data_dir>/
  crucible.db           # SQLite database
  repos/<engine>/       # clones of tracked repositories
  gates/*.json          # release gate summaries
  training/.../*.jsonl  # exported training data
```

The database holds engines, revisions, jobs, games, bisect sessions, training run metadata, and gate runs. Cancelled or interrupted jobs are re-queued when the daemon restarts, so nothing gets stuck in a stale "running" state.
