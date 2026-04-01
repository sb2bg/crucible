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

FROM debian:bookworm-slim

ARG ZIG_VERSION=0.15.2

RUN apt-get update && apt-get install -y \
    curl \
    git \
    libssl3 \
    ca-certificates \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

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
RUN mkdir -p /work/.crucible && chown -R crucible:crucible /work /home/crucible
USER crucible
EXPOSE 8877

ENTRYPOINT ["crucible"]
CMD ["--config", "/work/crucible.toml", "run"]
