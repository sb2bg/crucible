---
title: Engine runtimes
nav_order: 7
---

# Engine runtimes

Crucible is language-agnostic only after your engine can be launched as a UCI process. For every revision it tests, Crucible clones the engine repository, runs `build_cmd` inside that clone, then starts the file named by `binary_path`.

For a local Cargo install, `build_cmd` runs on the host and can use whatever compilers, SDKs, libraries, and environment variables you already use to build the engine. For Docker, those same tools must exist inside the Crucible container. The published image includes common Rust, C/C++, Zig, .NET/C#, Java/Maven, JavaScript/npm, and Python/pip tools, but it does not include every possible chess-engine toolchain.

## Docker defaults

The published image includes:

| Runtime/toolchain | Included tools                                            |
| ----------------- | --------------------------------------------------------- |
| Rust              | `rustc`/`cargo` via rustup, default toolchain `1.94.1`    |
| C/C++             | `build-essential`, `clang`, `cmake`, `make`, `pkg-config` |
| Zig               | Zig `0.15.2`                                              |
| .NET/C#           | .NET SDK 8                                                |
| Java              | `openjdk-17-jdk-headless`, `maven`                        |
| JavaScript        | `nodejs`, `npm`                                           |
| Python            | `python3`, `python3-pip`, `python3-venv`                  |

Engines that need host-specific libraries, unusual compiler installs, several incompatible runtimes, or a different language version are often simpler with a local Crucible binary. If you want to keep Docker, use a custom image, mounted toolchain, or a separate Crucible instance. See [Docker](docker.md) for full container setup patterns.

## The rule

`binary_path` must be an executable path relative to the cloned repository root:

```toml
[[engines]]
name = "my-engine"
repo = "https://github.com/you/my-engine"
branches = ["main"]
build_cmd = "make"
binary_path = "my-engine"
```

Do not put an interpreter command in `binary_path`:

```toml
# Not valid
binary_path = "java -jar target/my-engine.jar"
```

For engines that need an interpreter or runtime, make `build_cmd` create a small executable wrapper script and point `binary_path` at that wrapper.

## Custom Docker images

If the published image does not have the tools you need, derive a project-specific image from it.

Create `Dockerfile.crucible` beside your `crucible.toml`:

```dockerfile
FROM ghcr.io/sb2bg/crucible:latest

USER root

RUN apt-get update && apt-get install -y --no-install-recommends \
    ninja-build \
    && rm -rf /var/lib/apt/lists/*

USER crucible
```

Then point Compose at it:

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

For Rust engines that need a different default Rust version, use the matching Rust image as the base and copy the Crucible binary from the published image:

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

For Haskell, different Rust or .NET versions, or other large ecosystems, the same pattern applies: start from an image that has the toolchain, copy `/usr/local/bin/crucible` into it, and keep the same `/work` volume layout.

You can also mount a portable toolchain directory into the standard image:

```yaml
services:
  crucible:
    image: ghcr.io/sb2bg/crucible:latest
    environment:
      PATH: /opt/toolchains/my-compiler/bin:/usr/local/bin:/usr/bin:/bin
    volumes:
      - ./crucible.toml:/work/crucible.toml:ro
      - ./toolchains/my-compiler:/opt/toolchains/my-compiler:ro
      - crucible-data:/work/.crucible
```

Prefer mounting a complete, portable toolchain directory over bind-mounting individual host compiler binaries. Individual binaries usually depend on matching headers, linkers, libraries, and distro paths.

## Language examples

### C or C++

The published image includes common C/C++ build tools.

```toml
[[engines]]
name = "my-cpp-engine"
repo = "https://github.com/you/my-cpp-engine"
branches = ["main"]
build_cmd = "cmake -S . -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build -j"
binary_path = "build/my-cpp-engine"
```

### Rust

The published image includes Rust `1.94.1` through rustup.

```toml
[[engines]]
name = "my-rust-engine"
repo = "https://github.com/you/my-rust-engine"
branches = ["main"]
build_cmd = "cargo build --release --locked"
binary_path = "target/release/my-rust-engine"
```

If your engine has a `rust-toolchain.toml`, rustup will honor it. For engines pinned to a very different toolchain line, a custom image keeps builds more reproducible.

### Zig

The published image includes Zig `0.15.2`. Zig is pinned because many Zig releases introduce breaking language or build-system changes.

```toml
[[engines]]
name = "my-zig-engine"
repo = "https://github.com/you/my-zig-engine"
branches = ["main"]
build_cmd = "zig build -Doptimize=ReleaseFast"
binary_path = "zig-out/bin/my-zig-engine"
```

### Java

The published image includes a Java 17 JDK and Maven. If the project builds a JAR, create an executable wrapper in `build_cmd`.

```toml
[[engines]]
name = "my-java-engine"
repo = "https://github.com/you/my-java-engine"
branches = ["main"]
build_cmd = """
mvn package
cat > target/crucible-engine <<'EOF'
#!/usr/bin/env sh
exec java -jar "$(dirname "$0")/my-engine.jar"
EOF
chmod +x target/crucible-engine
"""
binary_path = "target/crucible-engine"
```

### C# / .NET

The published image includes .NET SDK 8. Use a custom image if your engine needs a different SDK line, or publish a self-contained executable.

```toml
[[engines]]
name = "my-dotnet-engine"
repo = "https://github.com/you/my-dotnet-engine"
branches = ["main"]
build_cmd = "dotnet publish -c Release -r linux-x64 --self-contained true -o out"
binary_path = "out/MyEngine"
```

### Python

The published image includes Python 3, pip, and venv support. Create a wrapper script if your engine is launched through the interpreter.

```toml
[[engines]]
name = "my-python-engine"
repo = "https://github.com/you/my-python-engine"
branches = ["main"]
build_cmd = """
cat > crucible-engine <<'EOF'
#!/usr/bin/env sh
exec python3 "$(dirname "$0")/engine.py"
EOF
chmod +x crucible-engine
"""
binary_path = "crucible-engine"
```

### JavaScript / Node.js

The published image includes Node.js and npm.

```toml
[[engines]]
name = "my-js-engine"
repo = "https://github.com/you/my-js-engine"
branches = ["main"]
build_cmd = """
npm ci
cat > crucible-engine <<'EOF'
#!/usr/bin/env sh
exec node "$(dirname "$0")/engine.js"
EOF
chmod +x crucible-engine
"""
binary_path = "crucible-engine"
```

### Haskell

Install `ghc` and `cabal` or `stack` in the image. Copy the built executable to a stable path.

```toml
[[engines]]
name = "my-haskell-engine"
repo = "https://github.com/you/my-haskell-engine"
branches = ["main"]
build_cmd = "cabal update && cabal build exe:my-engine && cp $(cabal list-bin exe:my-engine) ./my-engine"
binary_path = "my-engine"
```

## What Crucible does not do yet

Crucible does not yet run each engine build in a separate Docker image, select a Docker image per engine, or install toolchains dynamically. All tracked engines share the same host/container environment.

If you track engines with incompatible toolchain needs, run separate Crucible instances with separate `data_dir` values and different custom images.
