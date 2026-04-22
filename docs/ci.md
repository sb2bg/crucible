---
title: CI and releases
nav_order: 16
---

# CI and releases

Crucible ships two GitHub Actions workflows. `ci.yml` (filed as `docker-publish.yml` for historical reasons) runs on every push and pull request. `release.yml` runs when a `v*` tag is pushed.

## Tests

The `ci` workflow runs three jobs on every pull request and every push to `main` or a `v*` tag.

**`test (pinned)`** uses the toolchain pinned in `rust-toolchain.toml` (currently 1.94.1) and runs:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features --locked -- -D warnings`
- `cargo check --locked`
- `cargo test --locked`

**`msrv (1.88)`** verifies that Crucible still builds against the minimum supported Rust version declared in `Cargo.toml`. The pinned toolchain file is removed before `rustup` installs the MSRV toolchain, so this job actually exercises 1.88 rather than the pinned version.

**`docker`** builds the image on every event and pushes it to GitHub Container Registry at `ghcr.io/sb2bg/crucible` on every non-PR event.

Cargo artifacts are cached with `Swatinem/rust-cache` and Docker layers with `type=gha`, so repeated runs are fast.

## Docker images

Tags applied to pushed images:

- `edge` on pushes to `main`. This is the bleeding-edge image that tracks the default branch.
- `latest` on `v*` tags. This only ever points at the most recent published release.
- `type=semver,pattern={{version}}` and `{{major}}.{{minor}}` for `v*` tags, so you can pin to a specific release or a minor line.
- `type=ref,event=branch` and `type=ref,event=pr` for non-main branches and pull requests, for easy ad-hoc testing.
- `type=sha` for every build.

The `Dockerfile` uses a two-stage build. The first stage compiles a release binary against `rust:1.94.1-bookworm`; the second stage copies it into a .NET SDK Debian base with common engine tools for Rust `1.94.1`, C/C++, Zig `0.15.2`, Java/Maven, JavaScript/npm, and Python/pip. Larger or version-sensitive SDKs such as Haskell and non-default Rust/.NET versions are left to custom images or mounted toolchains.

## Releases

Tagging a `v*` tag runs the `release` workflow, which:

1. Builds release binaries for Linux `x86_64`, macOS `x86_64`, macOS `aarch64`, and Windows `x86_64` from the same checked-in source.
2. Packages each binary into an archive (`.tar.gz` on Unix, `.zip` on Windows) along with `README.md`, `LICENSE`, and `CHANGELOG.md`.
3. Generates a SHA-256 checksum file next to each archive.
4. Creates a GitHub Release, attaches the archives and checksums, and uses the matching section of `CHANGELOG.md` as the release body.

Tags that contain a hyphen (for example `v1.0.0-rc.1`) are published as pre-releases.

## Cutting a release

The release process is deliberately short:

1. Update `CHANGELOG.md`: move the `Unreleased` section into a new `[X.Y.Z] - YYYY-MM-DD` heading, and start a fresh `Unreleased` block.
2. Bump the `version` field in `Cargo.toml`. Run `cargo check` to refresh `Cargo.lock`.
3. Commit the changelog and version bump, push to `main`, and wait for CI to go green.
4. Tag the release: `git tag -a vX.Y.Z -m "vX.Y.Z"` and push the tag.
5. The `release` workflow builds, packages, and publishes everything. The `ci` workflow also fires on the tag and pushes the Docker image with the matching semver tag and `latest`.
