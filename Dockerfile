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
    libaacs-dev \
    libbdplus-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

RUN cargo build --release

# --- Runtime stage ---
# Must match the builder's Debian version so shared library versions align.
FROM debian:trixie-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libavcodec61 \
    libavformat61 \
    libavutil59 \
    libavdevice61 \
    libavfilter10 \
    libswscale8 \
    libbluray2 \
    libssl3t64 \
    libaacs0 \
    libbdplus0 \
    eject \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/bdrip /usr/local/bin/bdrip

ENTRYPOINT ["bdrip"]
