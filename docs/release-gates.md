---
title: Release gates
nav_order: 11
---

# Release gates

A release gate answers the question "is this candidate revision good enough to tag?" by playing it against a fixed panel of external engines and comparing the result to the same panel run against a baseline revision.

## Defining a profile

Gate profiles live in `crucible.toml`. You name the opponents once under `[[gate.opponents]]` and then group them into profiles under `[[gate.profiles]]`:

```toml
[[gate.opponents]]
name = "Stockfish"
binary_path = "/opt/engines/stockfish"

[[gate.opponents]]
name = "Ethereal"
binary_path = "/opt/engines/ethereal"

[[gate.profiles]]
name = "release"
opponents = ["Stockfish", "Ethereal"]
games_per_opponent = 200
min_score_delta = 1.0
opening_book = "openings/gate.epd"

[gate.profiles.time_control]
base_ms = 30000
increment_ms = 300
```

`opening_book` and `time_control` are optional and override `testing.opening_book` and `testing.time_control` for the duration of the gate.

## Running a gate

```bash
crucible gate \
  --engine my-engine \
  --candidate exp/new-search \
  --baseline v1.0.0 \
  --profile release
```

Crucible plays `games_per_opponent` games between the candidate and each opponent, then the same number between the baseline and each opponent. It also runs a direct candidate-versus-baseline match under the same game budget. All three phases share the same opening book so the only thing that varies between candidate and baseline is the engine binary.

## The summary

The output is a JSON file under `data_dir/gates/` by default, or at `--output <path>` if you pass one. It contains:

- Candidate vs suite: wins, draws, losses, score percentage, Elo diff with standard error, and LOS.
- Baseline vs suite: the same fields.
- Head-to-head candidate vs baseline: the same fields plus the final SPRT verdict.
- Aggregate score delta in percentage points (candidate minus baseline).
- Verdict: `Pass`, `Fail`, or `Tie`, decided by comparing the score delta to `min_score_delta`.

The Gate tab on the dashboard lists all completed gates and lets you download each summary. Running gates also show up live, so you can cancel them from the UI if you need the workers back.

## Choosing `min_score_delta`

`min_score_delta` is measured in score percentage points across the whole suite. A value of `0.0` treats any improvement as passing; `1.0` requires a full percentage point of headroom. The right number depends on how many games you play: shorter gates have noisier scores, so the threshold should be higher to avoid false positives.
