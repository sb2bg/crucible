# Build stage
FROM rust:1.83-bookworm AS builder

WORKDIR /app

# Install dependencies needed by git2 and rusqlite
RUN apt-get update && apt-get install -y \
    cmake \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies by building them first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
RUN cargo build --release && rm -rf src

# Build the actual project
COPY src/ src/
COPY templates/ templates/
RUN touch src/main.rs src/lib.rs && cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    git \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/crucible /usr/local/bin/crucible

WORKDIR /work

ENTRYPOINT ["crucible"]
