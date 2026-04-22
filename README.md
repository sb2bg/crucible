# Crucible

[![ci (main)](https://github.com/sb2bg/crucible/actions/workflows/docker-publish.yml/badge.svg?branch=main)](https://github.com/sb2bg/crucible/actions/workflows/docker-publish.yml?query=branch%3Amain)
[![release](https://github.com/sb2bg/crucible/actions/workflows/release.yml/badge.svg)](https://github.com/sb2bg/crucible/actions/workflows/release.yml)
[![latest release](https://img.shields.io/github/v/release/sb2bg/crucible?display_name=tag&sort=semver)](https://github.com/sb2bg/crucible/releases/latest)
[![license](https://img.shields.io/github/license/sb2bg/crucible)](LICENSE)

[![Experiments screenshot](https://github.com/sb2bg/crucible/raw/main/docs/assets/experiments.png)](https://sb2bg.github.io/crucible/)

Crucible is continuous integration for chess engines. It watches your engine's git history, builds every commit, plays it against its predecessor under the Sequential Probability Ratio Test, and shows you an Elo timeline so you can see which changes made the engine stronger or weaker.

Existing platforms such as [OpenBench](https://github.com/AndyGrant/OpenBench) are designed for large teams running distributed tests across many volunteer machines. Crucible is for the solo developer who just wants to know whether the last handful of commits helped. Everything runs on one machine, from a single binary, backed by SQLite.

The full documentation lives at **<https://sb2bg.github.io/crucible>**, or under [`docs/`](docs/) in this repository.

## Quick start

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

For a Docker-based setup, see [Getting started](docs/getting-started.md).

## Features

- Continuous SPRT testing of every new commit against its predecessor.
- Elo timeline with confidence intervals and highlighted tagged releases.
- Regression hunts that narrow a good-to-bad range down to the first bad commit.
- Release gates that compare a candidate and baseline against the same external gauntlet.
- NNUE-style training data exported from self-play and from the regression tests the daemon already runs.
- Multi-engine, multi-branch support, with experimental branches kept in their own lane.
- Embedded web dashboard plus an optional terminal UI.
- A single binary, SQLite storage, and no external services.

## Documentation

- [Getting started](docs/getting-started.md)
- [Recommended workflow](docs/workflow.md)
- [Configuration reference](docs/configuration.md)
- [CLI commands](docs/commands.md)
- [Scheduling](docs/scheduling.md)
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
