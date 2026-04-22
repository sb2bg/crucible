---
title: CI and releases
nav_order: 13
---

# CI and releases

Crucible's own GitHub Actions workflow lives at `.github/workflows/docker-publish.yml`. It does two things.

## Tests

Every pull request and every push to `main` or a `v*` tag runs:

- `cargo fmt --all --check`
- `cargo check --locked`
- `cargo test --locked`

The toolchain is pinned to the version referenced in `rust-toolchain.toml`. Cargo artifacts are cached with `Swatinem/rust-cache` so repeated runs are fast.

## Docker images

Once tests pass, the workflow builds the Docker image for every event. On pushes to `main` and on `v*` tags, the image is also pushed to GitHub Container Registry at `ghcr.io/sb2bg/crucible`.

Tags applied by `docker/metadata-action`:

- `type=ref,event=branch` for branch pushes.
- `type=ref,event=pr` for pull requests.
- `type=semver,pattern={{version}}` and `{{major}}.{{minor}}` for `v*` tags.
- `type=sha` for every build.
- `latest` on the default branch.

The `Dockerfile` uses a two-stage build. The first stage compiles a release binary against `rust:1.94.1-bookworm`; the second stage copies it into a slim Debian base that also includes Zig, so the image can build Zig-based engines out of the box.

Images are cached in GitHub Actions cache via `type=gha`, so rebuilds after small changes usually hit the cache for the dependency layer.
