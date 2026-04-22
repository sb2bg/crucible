# Security policy

## Supported versions

Crucible is pre-1.0. Security fixes are applied to the `main` branch and included in the next release. If you are running an older release, please upgrade to the latest published tag before reporting a vulnerability against it.

| Version        | Supported |
| -------------- | --------- |
| `main`         | Yes       |
| Latest release | Yes       |
| Older releases | No        |

## Reporting a vulnerability

Please report vulnerabilities privately. Do **not** open a public GitHub issue for anything that looks security-sensitive.

The preferred channel is GitHub's [private vulnerability reporting](https://github.com/sb2bg/crucible/security/advisories/new).

A good report includes:

- A description of the issue and the impact.
- Minimal reproduction steps, ideally against a fresh checkout of `main`.
- Your assessment of severity and any suggested mitigations.
- Whether you intend to disclose publicly, and on what timeline.

## What to expect

- Acknowledgement within three business days.
- An initial assessment within seven days, including a rough timeline for a fix.
- A fix in the next release for confirmed vulnerabilities, with a GitHub Security Advisory published once the fix is available.
- Credit in the advisory if you would like it.

## Scope

In scope:

- The Crucible daemon, CLI, web dashboard, and TUI.
- The Docker image published to GHCR.
- The documentation site at <https://sb2bg.github.io/crucible>.

Out of scope:

- Vulnerabilities in the chess engines Crucible tests. Report those to the engine authors.
- Vulnerabilities in upstream dependencies that have no concrete impact on Crucible. Report those to the dependency.
- Denial of service by configuring Crucible to exhaust its own host (for example, unlimited concurrency). Crucible is a single-tenant tool and is not designed to be hostile-user-safe.
