---
title: Scheduling
nav_order: 9
---

# Scheduling

Crucible's scheduler decides which commit to test next. The priorities, from highest to lowest, are:

1. Active regression-hunt probes. A bisect is user-initiated and someone is usually waiting on the answer.
2. Manual tests queued through `crucible test` or the admin tab.
3. Branch heads. Testing the tip of every tracked branch keeps the Timeline current.
4. Tagged releases. These are likely to be referenced later, so they should have verdicts available.
5. Recent commits near the branch head that have not yet been tested.
6. Backfill work. Older commits are tested in the background so the full Timeline fills in over time.

The daemon polls every `testing.poll_interval_seconds`. When it finds new commits, they enter the queue with the right priority for their position in history. New pushes to a tracked branch therefore jump ahead of backfill work that was already queued.

## Experimental branches

Branches listed under `experimental_branches` for an engine are still built and tested, but Crucible does not thread them into the canonical history. Instead of pairing each experimental commit against its predecessor on the same branch, the scheduler tests the branch head against the current `main` head. That gives you a single, meaningful "is my experiment better than main" comparison without polluting the Timeline with every intermediate commit.

The dashboard hides experimental jobs from the Timeline and Jobs views and surfaces them under the dedicated Experiments tab.

## Concurrency

`testing.concurrency` controls how many matches run in parallel. The daemon spawns up to that many worker tasks. Each worker claims the next-highest-priority job from the queue and holds onto it until the SPRT concludes or the match hits `testing.max_games`. If `testing.progression_games` is set, canonical sequential jobs instead play exactly that many games; experimental and manual patch tests remain SPRT-based.

If `training.idle_selfplay` is enabled, worker slots that cannot find a test job pick up a short self-play batch instead, so the machine keeps producing training data during quiet periods.

## Recovery

If the daemon is killed while jobs are running, those jobs are re-queued on the next startup. Nothing is left in a stuck "running" state.
