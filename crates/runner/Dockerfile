# syntax=docker/dockerfile:1.7

FROM rust:1.93.1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --release -p frontend-forge-runner

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

WORKDIR /app
COPY --from=builder /app/target/release/frontend-forge-runner /usr/local/bin/frontend-forge-runner

ENTRYPOINT ["/usr/local/bin/frontend-forge-runner"]
