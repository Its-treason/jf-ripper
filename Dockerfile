# --- Build stage ---
FROM rust:latest AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    clang \
    libclang-dev \
    libavcodec-dev \
    libavformat-dev \
    libavutil-dev \
    libavdevice-dev \
    libavfilter-dev \
    libswscale-dev \
    libbluray-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

RUN cargo build --release

# --- Runtime stage ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libavcodec59 \
    libavformat59 \
    libavutil57 \
    libavdevice59 \
    libavfilter8 \
    libswscale6 \
    libbluray2 \
    libssl3 \
    libaacs0 \
    libbdplus0 \
    eject \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/bdrip /usr/local/bin/bdrip

ENTRYPOINT ["bdrip"]
