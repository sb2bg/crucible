# Crucible

[![ci (main)](https://github.com/sb2bg/crucible/actions/workflows/docker-publish.yml/badge.svg?branch=main)](https://github.com/sb2bg/crucible/actions/workflows/docker-publish.yml?query=branch%3Amain)
[![release](https://github.com/sb2bg/crucible/actions/workflows/release.yml/badge.svg)](https://github.com/sb2bg/crucible/actions/workflows/release.yml)
[![latest release](https://img.shields.io/github/v/release/sb2bg/crucible?display_name=tag&sort=semver)](https://github.com/sb2bg/crucible/releases/latest)
[![license](https://img.shields.io/github/license/sb2bg/crucible)](LICENSE)

[![Experiments screenshot](https://github.com/sb2bg/crucible/blob/main/assets/experiments.png?raw=true)](https://sb2bg.github.io/crucible/)

Crucible is continuous integration for chess engines. It watches your engine's git history, builds every commit, plays it against its predecessor under the Sequential Probability Ratio Test, and shows you an Elo timeline so you can see which changes made the engine stronger or weaker.

Existing platforms such as [OpenBench](https://github.com/AndyGrant/OpenBench) are designed for large teams running distributed tests across many volunteer machines. Crucible is for the solo developer who just wants to know whether the last handful of commits helped. Everything runs on one machine, from a single binary, backed by SQLite. See [Why Crucible exists](docs/motivation.md) for the longer version.

The full documentation lives at **<https://sb2bg.github.io/crucible>**, or under [`docs/`](docs/) in this repository.

## Quick start

Choose either Docker or a local Cargo install. Docker is convenient for a long-running daemon with a bundled build environment; Cargo is usually simpler when your engine already builds on the host or depends on unusual local toolchains.

With Docker:

```bash
docker run --rm \
  -v "$PWD:/work" \
  ghcr.io/sb2bg/crucible:latest init

# edit crucible.toml, then:
docker compose up -d
```

The web dashboard opens at <http://localhost:8877>. If you bind the dashboard outside the container with `web_host = "0.0.0.0"`, Crucible requires a real `server.admin_token`.

The published image includes common engine tools for Rust, Go, C/C++, Zig, .NET/C#, Java/Maven, JavaScript/npm, and Python/pip. Haskell, unusual SDK versions, host-specific dependencies, and several incompatible runtimes are often easier with a local install; Docker can still work with a custom image or mounted toolchain. See [Docker](docs/docker.md) and [Engine runtimes](docs/engine-runtimes.md).

With Cargo:

```bash
cargo install crucible-chess
```

This puts a `crucible` binary on your `PATH`; run `crucible init`, add your engine, then run `crucible run`. The crate is named `crucible-chess` because plain `crucible` is taken on crates.io; the binary, library, and command-line interface are unaffected.

Or build from source:

```bash
cargo build --release

./target/release/crucible init

./target/release/crucible add \
  --name my-engine \
  --repo https://github.com/you/your-engine \
  --build "make" \
  --binary-path "target/release/my-engine" \
  --branches main,dev \
  --start-from v1.0.0

./target/release/crucible run
```

The web dashboard opens at <http://localhost:8877>. Pass `--tui` to launch the terminal UI alongside the daemon, or run `crucible monitor` in another shell to attach one to a running instance.

## Features

- Continuous commit-to-commit testing, with fixed-game Elo progression or SPRT stopping.
- Elo timeline with confidence intervals and highlighted tagged releases.
- Regression hunts that narrow a good-to-bad range down to the first bad commit.
- Release gates that compare a candidate and baseline against the same external gauntlet.
- NNUE-style training data exported from self-play and from the regression tests the daemon already runs.
- Multi-engine, multi-branch support, with [experimental branches](docs/experiments.md) kept in their own lane.
- Embedded web dashboard plus an optional terminal UI.
- A single binary, SQLite storage, and no external services.

## Documentation

- [Why Crucible exists](docs/motivation.md)
- [Getting started](docs/getting-started.md)
- [Docker](docs/docker.md)
- [Recommended workflow](docs/workflow.md)
- [Configuration reference](docs/configuration.md)
- [Engine runtimes](docs/engine-runtimes.md)
- [CLI commands](docs/commands.md)
- [Scheduling](docs/scheduling.md)
- [Experiments](docs/experiments.md)
- [Regression hunts](docs/regression-hunts.md)
- [Release gates](docs/release-gates.md)
- [Training data](docs/training-data.md)
- [Exporting results](docs/export.md)
- [Dashboards](docs/dashboards.md)
- [Architecture](docs/architecture.md)
- [CI and releases](docs/ci.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup, testing expectations, and project scope. Security issues should follow the private reporting process in [SECURITY.md](SECURITY.md). Notable changes are tracked in [CHANGELOG.md](CHANGELOG.md).

## License

GPL-3.0. See [LICENSE](LICENSE).
