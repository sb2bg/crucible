---
title: Docker
nav_order: 4
---

# Docker

Docker is one of two first-class ways to run Crucible. It is convenient for a long-lived daemon on a server, NAS, VPS, or desktop because the container keeps the SQLite database, cloned repositories, and build artifacts in a named volume, while your `crucible.toml` stays in your project directory.

Use Docker when your engines build with the bundled toolchains or with one stable custom image. If your engine depends on host-specific libraries, unusual compiler installs, GPUs, or several incompatible runtime environments, installing Crucible as a local binary is usually simpler. Docker can still work in those cases, but you will probably need a custom image, mounted toolchain, or separate Crucible instances.

## Quick start

Create a config:

```bash
docker run --rm \
  -v "$PWD:/work" \
  ghcr.io/sb2bg/crucible:latest init
```

If you bind the dashboard outside the container, set a real admin token:

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

Then run Crucible with Compose:

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

Start it:

```bash
docker compose up -d
```

The dashboard is available on the host port you publish, usually <http://localhost:8877>.

## Included toolchains

The published image is meant to cover common engine setups without becoming a full development workstation. It includes:

| Runtime/toolchain | Packages |
| ----------------- | -------- |
| Rust              | `rustc`/`cargo` via rustup, default toolchain `1.94.1` |
| Go                | Go `1.26.2` |
| C/C++             | `build-essential`, `clang`, `cmake`, `make`, `pkg-config` |
| Zig               | Zig `0.15.2` |
| .NET/C#           | .NET SDK 8 |
| Java              | `openjdk-17-jdk-headless`, `maven` |
| JavaScript        | `nodejs`, `npm` |
| Python            | `python3`, `python3-pip`, `python3-venv` |

It does not include every ecosystem. In particular, Haskell, host-specific dependencies, and engines that need a different SDK version may be simpler with a local binary. If you want to keep Docker, use a custom image or mounted toolchain. Zig is pinned to `0.15.2` because Zig releases often make breaking language and build-system changes.

## Custom images

If your engine needs tools that are not in the default image, derive your own image.

```dockerfile
FROM ghcr.io/sb2bg/crucible:latest

USER root

RUN apt-get update && apt-get install -y --no-install-recommends \
    gfortran \
    ninja-build \
    && rm -rf /var/lib/apt/lists/*

USER crucible
```

Point Compose at it:

```yaml
services:
  crucible:
    build:
      context: .
      dockerfile: Dockerfile.crucible
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

For Rust engines that need a different default Rust version, start from the matching Rust image and copy the Crucible binary into it:

```dockerfile
FROM ghcr.io/sb2bg/crucible:latest AS crucible

# Replace this tag with the Rust line your engine requires.
FROM rust:1.88-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=crucible /usr/local/bin/crucible /usr/local/bin/crucible

RUN useradd --create-home --uid 10001 crucible
WORKDIR /work
RUN mkdir -p /work/.crucible && chown -R crucible:crucible /work /home/crucible
USER crucible

EXPOSE 8877
ENTRYPOINT ["crucible"]
CMD ["--config", "/work/crucible.toml", "run"]
```

For .NET/C# engines that need a different SDK version, use the matching official SDK base and copy Crucible into it:

```dockerfile
FROM ghcr.io/sb2bg/crucible:latest AS crucible

# Replace this tag with the SDK line your engine requires.
FROM mcr.microsoft.com/dotnet/sdk:8.0-bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    git \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=crucible /usr/local/bin/crucible /usr/local/bin/crucible

RUN useradd --create-home --uid 10001 crucible
WORKDIR /work
RUN mkdir -p /work/.crucible && chown -R crucible:crucible /work /home/crucible
USER crucible

EXPOSE 8877
ENTRYPOINT ["crucible"]
CMD ["--config", "/work/crucible.toml", "run"]
```

## Mounted toolchains

You can also mount a portable toolchain into the container. This is useful when you already have a self-contained compiler bundle checked out beside your config.

```yaml
services:
  crucible:
    image: ghcr.io/sb2bg/crucible:latest
    restart: unless-stopped
    ports:
      - "8877:8877"
    command: ["--config", "/work/crucible.toml", "run"]
    environment:
      PATH: /opt/toolchains/my-compiler/bin:/usr/local/bin:/usr/bin:/bin
    volumes:
      - ./crucible.toml:/work/crucible.toml:ro
      - ./toolchains/my-compiler:/opt/toolchains/my-compiler:ro
      - crucible-data:/work/.crucible

volumes:
  crucible-data:
```

Prefer mounting a complete toolchain directory over bind-mounting individual host binaries such as `/usr/bin/gcc`. Individual compiler binaries usually depend on matching headers, linkers, libraries, and distro paths.

## Nested Docker builds

Advanced users can make `build_cmd` run another Docker container by mounting the Docker socket into Crucible. This can support per-engine build images, but it gives the Crucible container control over the host Docker daemon.

```yaml
volumes:
  - /var/run/docker.sock:/var/run/docker.sock
```

Treat this as a privileged setup. Prefer custom images or mounted portable toolchains when possible.

## Multiple incompatible engines

All engines in one Crucible instance share the same container environment. If you track engines with incompatible runtime needs, run separate Crucible instances with separate `data_dir` values and different images.
