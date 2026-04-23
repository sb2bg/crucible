---
title: Training data
nav_order: 13
---

# Training data

Crucible can produce NNUE-style JSONL training data from two sources: dedicated self-play runs and the regression matches the daemon already runs. Both are written under `training.output_dir`.

## Self-play runs

```bash
crucible selfplay-data --engine my-engine --games 200
```

By default the command picks the latest successfully built revision, reuses the configured time control, and writes to `training.output_dir/<engine>/<revision>/<run-timestamp>/`. Each run is split into files named by reported search depth, such as `depth-010.jsonl` and `depth-011.jsonl`.

Self-play runs keep only positions reported at exactly `training.selfplay_depth`. If you set `selfplay_depth = 10`, a self-play export contains pure D10 data.

To pin a revision or redirect the output:

```bash
crucible selfplay-data \
  --engine my-engine \
  --revision a1b2c3d \
  --games 500 \
  --depth 12 \
  --output-dir /data/nnue
```

## Collection from regression tests

When `training.collect_from_tests = true`, every regression match the daemon runs also writes JSONL alongside its results. These runs are separate from self-play: they keep positions at or above `training.regression_min_depth`, so with `regression_min_depth = 10` you still get usable D10+ data out of matches that the daemon was going to run anyway.

Each regression match produces two training runs, one for the dev side and one for the base side, so you can filter data by revision without mixing opponents.

## Idle self-play

Set `training.idle_selfplay = true` to let free worker slots generate short self-play batches whenever there are no queued test jobs. `idle_batch_games` controls how many games each batch plays. This is the easiest way to keep the machine busy between test waves.

## File layout

```
<output_dir>/
  <engine-name>/
    <revision-hash>/
      <run-timestamp>/
        depth-010.jsonl
        depth-011.jsonl
        ...
```

Every row in every JSONL file records one position. The schema includes:

- engine name and revision hash
- FEN and side to move
- reported search depth
- reported score for that side
- the move chosen at that position
- final game result from that side's perspective

The Training tab on the dashboard summarises self-play export runs and depth-bucket counts so you can see what data you have without diving into the filesystem.
