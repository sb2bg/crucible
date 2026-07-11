---
title: Getting started
nav_order: 3
---

# Getting started

Crucible is a single-node daemon for chess-engine testing. There are two first-class ways to run it: Docker, which gives you a bundled long-running container environment, and a local Cargo install, which uses the toolchains already available on the host.

Use Docker when you want a self-contained daemon on a server, NAS, VPS, or desktop and your engines build with the bundled toolchains or one stable custom image. Use a local binary when your engine already builds on the host, needs host-specific libraries, or depends on several incompatible runtimes.

## Option 1: Docker

A published image is available at `ghcr.io/sb2bg/crucible`. Generate a config in your current directory:

```bash
docker run --rm \
  -v "$PWD:/work" \
  ghcr.io/sb2bg/crucible:latest init
```

If you bind the dashboard outside the container, set `web_host = "0.0.0.0"` and a real `admin_token`:

```bash
openssl rand -hex 32
```

```toml
[server]
web_host = "0.0.0.0"
web_port = 8877
admin_token = "paste-generated-token-here"
```

The literal placeholder above is rejected at startup. Replace it with the generated value before you expose the dashboard.

Create `docker-compose.yml`:

```yaml
services:
  crucible:
    image: ghcr.io/sb2bg/crucible:latest
    restart: unless-stopped
    ports:
      - "8877:8877"
    command: ["--config", "/work/crucible.toml", "run"]
    volumes:
      - ./crucible.toml:/work/crucible.toml:ro
      - crucible-data:/work/.crucible

volumes:
  crucible-data:
```

Then start the stack:

```bash
docker compose up -d
```

The Compose file mounts `crucible.toml` read-only and uses a named Docker volume at `/work/.crucible` for the SQLite database, cloned repositories, and build artifacts.

The published image includes common engine tools for Rust, Go, C/C++, Zig, .NET/C#, Java/Maven, JavaScript/npm, and Python/pip. For Haskell, mounted toolchains, unusual SDK versions, host-specific dependencies, or per-project additions, see [Docker](docker.md) and [Engine runtimes](engine-runtimes.md).

## Option 2: Cargo

Install from crates.io:

```bash
cargo install crucible-chess
```

This drops a `crucible` executable on your `PATH`. The crate is named `crucible-chess` because plain `crucible` is taken on crates.io; the binary, library, and command-line interface are unaffected.

## Configure an engine

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

## Run the daemon

If you installed or built the binary locally, start the continuous testing loop:

```bash
crucible run
```

The daemon clones the repository, enumerates commits on the tracked branches, builds each one with your build command, and schedules matches between consecutive commits. With `progression_games` set, canonical history uses equal fixed-game samples for a more comparable Elo timeline; experimental branches and manual patch tests use SPRT. The web dashboard opens at <http://localhost:8877> by default.

Pass `--tui` to the same command to launch the terminal UI alongside the daemon, or run `crucible monitor` in another shell to attach a TUI to an already running instance.

## Build from source

Clone the repository and build a release binary:

```bash
git clone https://github.com/sb2bg/crucible
cd crucible
cargo build --release
```

The binary lands at `target/release/crucible`. Crucible targets Rust 1.88 or newer. The checked-in `rust-toolchain.toml` pins a compatible version, so `cargo` will pick it up without any extra setup.

## Exposing the dashboard

If you put the dashboard on a public network, place it behind an authentication layer such as Cloudflare Access, Tailscale, or a reverse proxy with access control. Crucible also requires `server.admin_token` whenever `server.web_host` is not loopback. The browser client stores the token in local storage until you clear it.

## Where to go next

- [Configuration reference](configuration.md) for every option in `crucible.toml`.
- [Docker](docker.md) for long-running container setups.
- [Engine runtimes](engine-runtimes.md) for compiler and runtime setup.
- [CLI commands](commands.md) for running one-off tests, regression hunts, and release gates.
- [Dashboards](dashboards.md) for a tour of the web UI and the terminal UI.
