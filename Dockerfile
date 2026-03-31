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

RUN apt-get update && apt-get install -y \
    git \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 crucible

COPY --from=builder /app/target/release/crucible /usr/local/bin/crucible

WORKDIR /work
USER crucible
EXPOSE 8877

ENTRYPOINT ["crucible"]
CMD ["run", "--config", "/work/crucible.toml"]
