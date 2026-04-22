# Contributing to Crucible

Thanks for your interest in Crucible. This document covers everything you need to know to get the code building, make a change, and get it reviewed.

## Scope

Crucible is a single-machine, solo-developer SPRT runner for chess engines. Contributions that improve that use case are very welcome:

- Bug fixes, especially around the scheduler, match runner, SPRT logic, or git handling.
- New engine build workflows (different languages, build systems, or platforms).
- Dashboard and TUI improvements.
- Documentation and examples.
- Performance work with a clear before/after measurement.

Contributions that push Crucible toward being a distributed testing platform are out of scope. If you need that, [OpenBench](https://github.com/AndyGrant/OpenBench) is the right tool.

For anything non-trivial, please open an issue first to discuss the approach before you start writing code. It saves everyone time.

## Development setup

Crucible is a standard Cargo project. The only hard requirement is a working Rust toolchain. The `rust-toolchain.toml` file pins the version used by the maintainers; `rustup` will pick it up automatically when you run `cargo` inside the repository.

```bash
git clone https://github.com/sb2bg/crucible
cd crucible

cargo build --release
cargo test --locked
```

Running Crucible locally against a test engine is the fastest way to find regressions in behaviour that the unit tests do not cover. The easiest path:

```bash
cargo run -- init
cargo run -- add --name test-engine --repo <some-small-engine-repo> --build "make" --binary-path "engine" --branches main
cargo run -- run
```

Open <http://localhost:8877> and watch the daemon do its thing.

## Before you open a PR

Please run all three of these locally. CI will run them anyway, but catching issues before push is faster for everyone:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --locked
```

If your change affects the database schema, note this in the PR description. Crucible does not yet have a migration framework, so schema changes need to be handled carefully.

If your change affects user-visible behaviour, update the relevant page under `docs/` and add an entry to `CHANGELOG.md` under the `Unreleased` section.

## Style and conventions

- Formatting is enforced by `cargo fmt`. The default `rustfmt` settings apply.
- Clippy runs with `-D warnings` in CI. Fix or suppress with a targeted `#[allow(...)]` and a one-line comment explaining why.
- Prefer small, focused PRs over one large one. Split refactoring from behaviour changes when you can.
- Write commit messages that explain the "why" more than the "what". The diff already shows the "what".
- Avoid unnecessary dependencies. Every crate we add is a crate we have to keep up with.

## Testing

Unit tests live alongside the code they cover. Integration-style tests that exercise the storage layer or the match runner live in the same modules, gated behind `#[cfg(test)]`.

When adding a new feature, aim for at least one test that exercises the happy path and one that exercises a failure mode. When fixing a bug, add a test that fails before your fix and passes after.

## Documentation

Documentation lives under `docs/` and is published to <https://sb2bg.github.io/crucible> via GitHub Pages on every push to `main`. The site uses Jekyll with the just-the-docs theme. Each page has YAML frontmatter with `title` and `nav_order`.

To preview the site locally:

```bash
cd docs
bundle install
bundle exec jekyll serve
```

Open <http://localhost:4000> to view the rendered site.

If you are adding a new doc page, pick the next free `nav_order` value and add a link from `docs/index.md` or the relevant sibling page so it is reachable.

## Reporting bugs

Please use the bug report template when filing an issue. Include:

- Your operating system and Rust toolchain version.
- The exact command you ran and the exact error you saw (or the behaviour you observed).
- Minimal steps to reproduce, ideally against a public engine repository or a tiny reproducer.
- Relevant config (`crucible.toml`), with any secrets redacted.

For feature requests, use the feature request template and explain the use case you have in mind, not just the mechanism. "I want X because I am trying to do Y" is much easier to respond to than "please add X".

## Security

Do not file security issues as public bug reports. See [SECURITY.md](SECURITY.md) for the private reporting process.

## License

By contributing, you agree that your contribution will be licensed under the same GPL-3.0 license that covers the rest of the project. You retain copyright on your changes.
