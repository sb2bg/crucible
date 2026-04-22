---
title: Home
layout: home
nav_order: 1
---

# Crucible

Crucible is continuous integration for chess engines. It watches your engine's git history, builds every commit, plays it against its predecessor under the Sequential Probability Ratio Test, and plots an Elo timeline so you can see which changes made the engine stronger or weaker.

Most chess engine testing platforms, such as [OpenBench](https://github.com/AndyGrant/OpenBench), are built for large teams running tests across many volunteer machines. Crucible is built for the solo developer who just wants to know whether the last handful of commits helped. Everything runs on one machine, from a single binary, backed by SQLite.

## What it does

- Tests every new commit against the one before it, stopping as soon as SPRT reaches a verdict.
- Tracks several engines and branches at the same time and keeps experimental branches in their own lane.
- Prioritises branch heads and tagged releases, then backfills the rest of the history in the background.
- Hunts regressions by sampling a known-good to known-bad range and narrowing in on the first bad window.
- Gates releases by playing a candidate and a baseline against the same external gauntlet and reporting the score delta.
- Exports NNUE-style training data from self-play runs and from the regression tests it already runs.
- Ships an embedded web dashboard and an optional terminal UI.

## Where to go next

1. Start with [Getting started](getting-started.md) to install Crucible and set up your first engine.
2. Read the [Recommended workflow](workflow.md) for how to host the daemon and use it day to day.
3. Keep the [configuration reference](configuration.md) open while you edit `crucible.toml`.
4. Look at [CLI commands](commands.md) when you want to queue a one-off test, start a regression hunt, or run a release gate.
5. Read [Architecture](architecture.md) if you want to understand how the pieces fit together.

The source and issue tracker are at [github.com/sb2bg/crucible](https://github.com/sb2bg/crucible).
