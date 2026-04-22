# syntax=docker/dockerfile:1

FROM rust:1.94.1-bookworm AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y \
    cmake \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
RUN cargo build --release --locked && rm -rf src

COPY src/ src/
COPY templates/ templates/
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

FROM mcr.microsoft.com/dotnet/sdk:8.0-bookworm-slim

ARG ZIG_VERSION=0.15.2
ARG RUST_VERSION=1.94.1

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    clang \
    cmake \
    make \
    pkg-config \
    curl \
    git \
    libssl3 \
    ca-certificates \
    openjdk-17-jdk-headless \
    maven \
    nodejs \
    npm \
    python3 \
    python3-pip \
    python3-venv \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN set -eux; \
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh; \
    sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain "${RUST_VERSION}"; \
    rm /tmp/rustup-init.sh; \
    rustc --version; \
    cargo --version

RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) zig_arch="x86_64" ;; \
      arm64) zig_arch="aarch64" ;; \
      *) echo "unsupported architecture: $arch" >&2; exit 1 ;; \
    esac; \
    curl -L "https://ziglang.org/download/${ZIG_VERSION}/zig-${zig_arch}-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz; \
    tar -xf /tmp/zig.tar.xz -C /opt; \
    ln -s "/opt/zig-${zig_arch}-linux-${ZIG_VERSION}/zig" /usr/local/bin/zig; \
    rm /tmp/zig.tar.xz

RUN useradd --create-home --uid 10001 crucible

COPY --from=builder /app/target/release/crucible /usr/local/bin/crucible

WORKDIR /work
RUN mkdir -p /work/.crucible && chown -R crucible:crucible /work /home/crucible "${RUSTUP_HOME}" "${CARGO_HOME}"
USER crucible
EXPOSE 8877

ENTRYPOINT ["crucible"]
CMD ["--config", "/work/crucible.toml", "run"]
