---
title: Experiments
nav_order: 10
---

# Experiments

An experiment in Crucible is a branch that tests one idea against your current `main`. Unlike a commit on `main`, an experimental commit is not part of the canonical history. Its verdict answers "is this idea better than main?", not "is this commit better than the one before it?". In addition, it is intended for changes where you are unsure of elo effect, and don't want to pollute the canonical history. That difference changes how the scheduler pairs matches, how the Experiments tab reads, and how you decide when to merge.

This page is about using experiments day-to-day. For the scheduler mechanics, see [Scheduling](scheduling.md). For the tab that displays them, see [Dashboards](dashboards.md).

## The mental model

Your engine has a `main` branch representing the current best version. Everything on `main` is something you have already accepted. When you have an idea that might make the engine stronger, you branch off and try it:

```
main:     A --- B --- C --- D --- E (HEAD)
                                \
exp/idea:                        F --- G --- H (HEAD)
```

On `main`, the scheduler pairs each commit against its predecessor: `B vs A`, `C vs B`, and so on. That is how the Timeline accumulates.

On an experimental branch, the scheduler does not pair `G vs F` or `H vs G`. It pairs the experimental HEAD against `main` HEAD: `H vs E`. That is the only comparison that answers the question you actually care about.

If you run three experiments in parallel, you get three independent "this idea vs main" verdicts, not three tangled histories fighting for screen space.

## Setup

Mark which branches are experimental in `crucible.toml`:

```toml
[[engines]]
name = "my-engine"
repo = "https://github.com/you/your-engine"
branches = ["main"]
experimental_branches = ["exp/*"]
build_cmd = "make"
binary_path = "target/release/my-engine"
```

Wildcards are matched against `origin/*`, so `exp/*` picks up every remote branch with that prefix. A common convention is `exp/<short-name>` per hypothesis: `exp/null-move-scaling`, `exp/iir-off`, `exp/fp-improving`. Short, specific, one idea per branch.

You can list several patterns if you want more than one namespace:

```toml
experimental_branches = ["exp/*", "try/*", "wip/*"]
```

Everything matching those patterns shows up in the Experiments tab and stays out of the Timeline and Jobs views, so the canonical history is not drowned in half-tested ideas.

## The daily loop

1. **Create the branch.** `git checkout -b exp/my-idea` from the latest `main`, make your change, push. Crucible picks it up on the next poll.
2. **Wait for the first verdict.** The scheduler queues `exp HEAD vs main HEAD`. SPRT concludes as soon as it has enough games to decide, typically minutes to hours depending on your time control and the size of the effect.
3. **Read the verdict in the Experiments tab.** Each row shows W/D/L, Elo with error, LOS, and the SPRT result: `H1Accepted` (your idea is better), `H0Accepted` (no improvement), or `Inconclusive` (not enough signal within the games budget).
4. **Decide.**
   - `H1Accepted`: merge the branch into `main`. The next `main` build reflects the change, and the Timeline picks up the gain.
   - `H0Accepted`: kill the branch. Delete it locally and on the remote so it stops showing up in the tab.
   - `Inconclusive`: either keep iterating (push more commits to the same branch, which re-queues the match against current `main`) or raise `testing.max_games` and re-queue.

If you push new commits to an experimental branch, Crucible does not keep the old verdict. It re-tests `exp HEAD vs main HEAD`, because that HEAD now represents a different idea. The chart in the Experiments tab keeps the per-commit history so you can see how the idea evolved, but the "official" verdict is always against the current HEAD.

## Promoting an experiment

When an experiment wins and you merge it, two things happen:

1. The commits land on `main` and become part of the canonical history. The next `main` build is the merged result, and the Timeline records it as any other commit.
2. The experimental branch, if you leave it around, now points at a commit that is already in `main`. Its "vs main" comparison collapses to a draw by construction.

Delete the merged branch to keep the Experiments tab clean. If you prefer to keep it for reference, that is fine, but you will see a stale "0 Elo, draw" row for it. That is cosmetic, not a real signal.

## Killing experiments

Experiments that do not work are not failures. They are the point. The whole reason for running an SPRT gauntlet on an idea is to find out cheaply that it does not help, so you can move on. Most experienced engine authors kill more experiments than they merge.

A practical rule: if an experiment comes back `H0Accepted` with a reasonable number of games played, delete the branch. Do not rerun it with looser SPRT bounds hoping for a different answer. The cost of a dead experiment is measured in branches and build time, not in developer sunk cost.

## Why the two lanes matter

The canonical `main` history answers "how has the engine changed over time?". The experiments view answers "what have I tried?". Both are useful, but they scale differently: you might have ten active experiments at once, most of which will be killed, and you do not want those showing up next to your actual releases. Keeping them in separate lanes is the only way to stop one from drowning out the other.
