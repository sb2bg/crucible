# Changelog

All notable changes to Crucible are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/sb2bg/crucible/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/sb2bg/crucible/releases/tag/v0.1.0
