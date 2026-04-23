---
title: Regression hunts
nav_order: 11
---

# Regression hunts

When the Timeline shows a drop, `crucible bisect` narrows the search to the single commit that caused it. The hunt runs in two phases.

## Phase 1: sample the range

Crucible enumerates every commit between the known-good and known-bad refs and picks a handful of sample points spread across the range. Each sample is tested against the known-good baseline using SPRT bounds tuned for detecting a regression, rather than for confirming an improvement. The match stops as soon as the test is conclusive, which in practice is much faster than playing a fixed-length gauntlet.

After the samples finish, Crucible picks the earliest window in which the engine is clearly worse than the baseline. That window is usually small, often a handful of commits.

## Phase 2: narrow to the culprit

Inside the shortlisted window Crucible runs a standard bisect: the midpoint is tested against the baseline, and the window shrinks to the half that still shows the regression. The loop ends when only one commit remains. Crucible records that commit as the suspected culprit and marks the session as `Found`.

If a probe comes back inconclusive, the scheduler retries it rather than trusting a weak result. If a probe keeps failing to build or fails to run, the session is marked `Failed` and you can inspect the relevant job in the dashboard.

## Starting a hunt

```bash
crucible bisect --engine my-engine --good v1.0.0 --bad HEAD
```

Both refs are resolved by commit prefix, tag name, or full hash against the engine's revision table. If either ref cannot be found Crucible prints an error before scheduling anything.

Active hunts appear in the Bisect tab of the web dashboard, and in the TUI's Bisect tab. Cancelled jobs release their worker slot immediately.

## When a hunt will not help

A regression hunt assumes the drop is caused by a single commit. If the engine slowly got worse over many commits, or if the change in Elo is smaller than the SPRT bounds can resolve, the hunt will struggle to converge. In those cases, running longer matches at a few specific commits tends to give you a clearer answer than automated bisection.
