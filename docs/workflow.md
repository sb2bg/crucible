---
title: Recommended workflow
nav_order: 3
---

# Recommended workflow

Crucible is a daemon. It is designed to sit on one machine, watch a repository, and keep testing forever. That shape works equally well on a home server, a spare desktop, or a laptop, but the value compounds the longer it stays running, so pick the host you are happy leaving alone.

## Where to run it

### Always-on machine

The recommended setup is an always-on Linux host: a home server, a mini-PC under your desk, or a cheap VPS. You start the daemon once, leave the dashboard on a local URL you can open from anywhere on the network, and every push to your engine triggers fresh work without you doing anything. This is where Crucible earns its keep, because the backfill queue has time to catch up and the Timeline becomes a real historical record instead of a series of partial runs.

Running it under Docker with the provided Compose file is the easiest path, since the image already includes Zig and pins the SQLite volume for you. See [Getting started](getting-started.md) for the Compose configuration.

### Dedicated desktop

A desktop you already use for engine development is fine. Crucible only needs CPU when there are queued jobs, and `testing.concurrency` lets you cap how many parallel matches it runs so it does not fight your interactive work. A reasonable setup is to leave the daemon running on startup and set `concurrency` to half your physical cores.

### Laptop

Laptops work too. The daemon is designed to survive interruptions: if you sleep or kill it mid-match, any running jobs are re-queued on the next start, and the scheduler picks up where it left off. The only real downside is that the Timeline only advances while the laptop is awake, which makes backfill slower for engines with long histories.

If you develop on a laptop but have a server available, point both machines at the same repository and let the server run Crucible. You get uninterrupted testing without changing where you write code.

## The day-to-day loop

Once the daemon is running, the workflow looks the same regardless of host:

1. **Add the engine once.** Use `crucible add` or a `[[engines]]` block in `crucible.toml`. `start_from` is useful here so Crucible does not try to build your pre-history.
2. **Let it backfill.** The first hours or days are spent building old commits and filling the Timeline. The dashboard's Overview tab shows queue depth so you know when this is done.
3. **Watch the Timeline.** Open the web dashboard when you want to see the Elo curve. Tagged releases and branch heads are highlighted, so you can tell at a glance whether the last push helped.
4. **React to drops with bisect.** When the curve takes a dive, run `crucible bisect --good <tag> --bad HEAD` and let it narrow to the first bad commit. The hunt shares the worker pool, so it will not starve other work for long.
5. **Gate before you tag.** Before cutting a release, run `crucible gate` against your configured external engines. The JSON summary and pass/fail verdict are meant to be read directly, no post-processing required.
6. **Export occasionally.** `crucible export` gives you a single JSON bundle you can paste into scripts, share, or feed to an LLM. This is easier than querying the SQLite database by hand.

That is the entire loop. Most of the time you are writing engine code and Crucible is running in the background, producing answers you can glance at when you want them.

## When to use the CLI vs the dashboard

The CLI and the dashboard are both first-class. They share the same database, so anything you queue from one shows up in the other.

Reach for the CLI when you want to:

- Script something: run a bisect from a commit hook, export the JSON bundle on a schedule, kick off gates from release automation.
- Do a one-off test that you do not need to see visually.
- Work over SSH on the host where the daemon runs.

Reach for the dashboard when you want to:

- Explore. The Timeline, diff viewer, and per-job game lists are built for reading, not parsing.
- Compare two arbitrary revisions without thinking about flags.
- Cancel or re-queue jobs interactively.
- Check status from a different machine.

The terminal UI (`crucible monitor`) is a reasonable middle ground over SSH when you want more than a status line but do not want to tunnel the web port.

## A concrete starting recipe

For a new engine on a fresh server:

```bash
crucible init
# edit crucible.toml: set concurrency to your core count, add the engine

crucible add \
  --name my-engine \
  --repo https://github.com/you/your-engine \
  --build "make" \
  --binary-path "out/my-engine" \
  --branches main \
  --start-from v0.1.0

crucible run
```

Open <http://localhost:8877>, wait an hour, and you should start seeing the earliest commits land on the Timeline. After that, every push to `main` is tested automatically and the curve fills in over time.
