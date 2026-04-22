---
title: Getting started
nav_order: 2
---

# Getting started

Crucible is a single Rust binary. You can build it from source, run it under Docker, or leave it running as a long-lived service. This page walks through the first two paths.

## Building from source

Clone the repository and build a release binary:

```bash
git clone https://github.com/sb2bg/crucible
cd crucible
cargo build --release
```

The binary lands at `target/release/crucible`. The rest of this page assumes `crucible` is on your `PATH`, but you can invoke the binary directly if you prefer.

Crucible targets Rust 1.88 or newer. The checked-in `rust-toolchain.toml` pins a compatible version, so `cargo` will pick it up without any extra setup.

## Initialising a project

Choose a working directory for Crucible's data and configuration, then generate an example config:

```bash
crucible init
```

This writes `crucible.toml` in the current directory with a sensible starting set of options. The [configuration reference](configuration.md) covers every field you can tune.

Add an engine you want to track:

```bash
crucible add \
  --name my-engine \
  --repo https://github.com/you/your-engine \
  --build "make" \
  --binary-path "target/release/my-engine" \
  --branches main,dev \
  --start-from v1.0.0
```

You can skip the `add` step and list engines directly under `[[engines]]` in `crucible.toml` instead. Entries in the config file are imported when the daemon starts, so you can keep your engines under version control if you want to.

## Running the daemon

Start the continuous testing loop:

```bash
crucible run
```

The daemon clones the repository, enumerates commits on the tracked branches, builds each one with your build command, and schedules SPRT matches between consecutive commits. The web dashboard opens at <http://localhost:8877> by default.

Pass `--tui` to the same command to launch the terminal UI alongside the daemon, or run `crucible monitor` in another shell to attach a TUI to an already running instance.

## Running with Docker

A published image is available at `ghcr.io/sb2bg/crucible`. To run it with the bundled Compose file, first set `web_host = "0.0.0.0"` in your config so the dashboard is reachable from outside the container:

```toml
[server]
web_host = "0.0.0.0"
web_port = 8877
```

Then start the stack:

```bash
cargo run -- init
docker compose up --build -d
```

The Compose file mounts `crucible.toml` read-only and uses a named Docker volume at `/work/.crucible` for the SQLite database, cloned repositories, and build artifacts.

If you prefer a plain `docker run`:

```bash
docker build -t crucible .
docker run -d \
  --name crucible \
  -p 8877:8877 \
  -v "$(pwd)/crucible.toml:/work/crucible.toml:ro" \
  -v crucible-data:/work/.crucible \
  crucible
```

The Docker image also ships with Zig preinstalled, which is convenient if your engine is built with Zig rather than Rust or C.

## Exposing the dashboard

If you put the dashboard on a public network, place it behind an authentication layer such as Cloudflare Access, Tailscale, or a reverse proxy with access control. You can also set `server.admin_token` in your config to require a bearer token on every `/api/admin/*` route. The browser client stores the token in local storage until you clear it.

## Where to go next

- [Configuration reference](configuration.md) for every option in `crucible.toml`.
- [CLI commands](commands.md) for running one-off tests, regression hunts, and release gates.
- [Dashboards](dashboards.md) for a tour of the web UI and the terminal UI.
