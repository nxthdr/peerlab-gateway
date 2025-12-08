FROM lukemathwalker/cargo-chef:latest-rust-1-trixie AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apt-get update \
    && apt-get install -y libsasl2-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin peerlab-gateway

FROM debian:trixie-slim AS runtime
RUN apt-get update \
    && apt-get install -y libsasl2-dev libssl-dev ca-certificates \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/peerlab-gateway /app/peerlab-gateway

WORKDIR /app
EXPOSE 8080

ENTRYPOINT [ "/app/peerlab-gateway" ]
