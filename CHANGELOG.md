# Changelog

All notable changes to Crucible are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2] - 2026-07-10

### Added

- Go `1.26.2` to the default Docker engine toolchain image.
- Optional fixed-game canonical progression matches via `testing.progression_games`, while experimental and manual patch tests continue to use SPRT.

### Changed

- Release gates now always play their full, color-balanced configured game count instead of stopping early on an SPRT boundary.

### Fixed

- Preserve the daemon environment when launching engine build commands, preventing Docker builds from losing `/usr/local/cargo/bin` and failing with `cargo: not found`.
- Fail a job immediately when a game cannot be launched instead of repeating the same infrastructure error for the entire game budget.

## [0.1.1] - 2026-04-22

### Added

- Docker guide with Compose setup, admin-token guidance, mounted toolchains, custom images, and nested-Docker caveats.
- Engine runtime guide with examples for Rust, C/C++, Zig, .NET/C#, Java, Python, JavaScript, and Haskell engines.
- Motivation page explaining Crucible's single-machine workflow and how it differs from distributed engine-testing platforms.
- Docker image runtime toolchains for Rust `1.94.1`, C/C++, Zig `0.15.2`, .NET SDK 8, Java/Maven, JavaScript/npm, and Python/pip/venv.

### Changed

- Present Docker and Cargo/local installs as equal first-class setup paths, with guidance on when each is simpler.
- Use the published GHCR image in the bundled Compose file instead of building locally by default.
- Reorganize docs navigation so Docker and engine runtime setup have dedicated pages.
- Document that Zig is pinned to `0.15.2` because Zig releases often make breaking language and build-system changes.

### Security

- Reject `paste-generated-token-here` as an admin-token placeholder, matching the new Docker setup docs.

## [0.1.0] - 2026-04-22

### Added

- Release gate summaries now report Elo, standard error, and LOS for each side.
- `GET /api/health` endpoint that returns liveness without touching SQLite.
- Idle self-play batches that run on free worker slots when the test queue is empty.
- Published documentation site at <https://sb2bg.github.io/crucible>.
- Contribution guide, security policy, issue and PR templates.

### Changed

- Docker images tagged `edge` track the `main` branch; `latest` now points at the most recent release tag only.
- Opening books are now read as EPD files. Lines are FEN fragments followed by optional EPD operations; full six-field FEN lines are no longer accepted.
- Renamed the crate to `crucible-chess` for crates.io publishing. The binary and library names remain `crucible`, so `cargo install crucible-chess` still produces a `crucible` executable and `use crucible::...` imports continue to work unchanged.

### Fixed

- Interrupted jobs are re-queued on daemon restart instead of being stuck in the `Running` state.

### Security

- Require a non-placeholder `server.admin_token` when the dashboard binds to a non-loopback address.
- Protect training-run, revision-detail, and compare APIs with the admin token.
- Validate engine names and binary paths before storing or using them, and refuse build artifact copies or engine-data deletes outside managed repository directories.
- Redact engine build commands and binary paths from the public engine API.

[Unreleased]: https://github.com/sb2bg/crucible/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/sb2bg/crucible/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/sb2bg/crucible/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/sb2bg/crucible/releases/tag/v0.1.0
