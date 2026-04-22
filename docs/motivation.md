---
title: Why Crucible exists
nav_order: 2
---

# Why Crucible exists

[![Experiments tab](https://github.com/sb2bg/crucible/blob/main/assets/experiments.png?raw=true)](https://github.com/sb2bg/crucible/blob/main/assets/experiments.png?raw=true)

You pushed a commit that changed the search function. Did your engine get stronger or weaker?

Answering that question is way harder than it sounds. You can play a few hundred games against the previous version and eyeball the score, but a few hundred games is not enough to tell a real 3 Elo change from noise. The statistical tool for the job is the Sequential Probability Ratio Test, and SPRT has been the standard in computer chess for decades. SPRT isn't hard to implement, but running it continuously against your own git history takes infrastructure nobody builds for one person.

## What already exists

The serious tools in this space, [OpenBench](https://github.com/AndyGrant/OpenBench) and [Fishtest](https://tests.stockfishchess.org/), are designed for teams. A central coordinator hands out tests to volunteer machines, aggregates results, and publishes them on a leaderboard. For Stockfish-scale engines this is the right shape, because the test budget is huge, the community is active, and distributed workers are how you get days of games in hours.

None of that applies if you are one person. You do not want to stand up a web service, onboard testers, and write an auth layer. You just want to know whether the last five commits helped. Pointing OpenBench at yourself is like setting up a Hadoop cluster to sort a CSV.

## What solo developers do instead

Roll their own. I know because I did. The usual progression:

1. A shell script that invokes a historical runner against two binaries for a fixed number of games and archives the results.
2. A second script that rebuilds and saves both binaries when you point it at two commit hashes.
3. A folder full of json files containing the archived results.
4. Eventually, a bash loop that runs through recent commits overnight.
5. You give up on the bash loop because the feedback arrives hours after you have already moved on to a different change.

The issue is how tedious, time consuming, and cluttering this can become. You run it when you remember to. Results are a grep away at best and lost at worst. There is no timeline, no verdict next to each commit in your git log, no record of what went wrong when the engine drops 20 Elo over three weeks. You cannot bisect a regression because you do not have enough data points to bisect from.

## What Crucible does

One process on one machine. It clones your engine's repository, builds every commit on the branches you tell it to watch, and plays SPRT matches between consecutive commits. Every commit gets a verdict and goes on the timeline. When something regresses, you point `crucible bisect` at a known-good and known-bad commit and it narrows the window down to the first bad commit without supervision.

- Continuous SPRT across your git history.
- Regression hunts that narrow a good-to-bad range down to the first bad commit.
- Release gates that compare a candidate to a baseline against a fixed external gauntlet.
- A web dashboard and a terminal UI, both reading the same SQLite database.
- Training data export as a side effect of the matches you were running anyway.

Thats' it! Storage is SQLite. The web UI is embedded in the binary. There is no configuration server, no registration flow, or test coordinator. You run `crucible run`, it does its thing, you look at the Timeline when you want to know how the engine is doing.

## What Crucible is not

Crucible is not a replacement for OpenBench. It is a different tool for a different situation.

- If you run a team and want volunteer test time, use OpenBench.
- If you are working on Stockfish and need days of games at long time controls, use Fishtest.
- If you want a public leaderboard of community engines, none of the three tools fits, and that is fine because it is a different problem.

Crucible assumes you are one person testing one engine on one machine. Within that scope, it tries to make every commit have a verdict without you thinking about it. Outside that scope, reach for a different tool.

## What I would like you to try first

If you are on the fence, the cheapest thing to do is `crucible init` against a chess engine repo and leave the daemon running overnight. A timeline with real commits on it tends to be more convincing than any documentation. When you come back in the morning, you either see the curve and understand immediately why I built this, or you do not, and in that case your workflow probably does not need what Crucible offers.

If you decide to try it properly, [Getting started](getting-started.md) has the install steps and [Recommended workflow](workflow.md) walks through the daily loop.
