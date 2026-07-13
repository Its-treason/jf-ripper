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
    libdvdread-dev \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

RUN cargo build --release

# --- libdvdcss stage ---
# Not in Debian main; built from source for CSS-encrypted DVD support.
# libdvdread dlopens libdvdcss.so.2 at runtime.
FROM debian:trixie-slim AS dvdcss

RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc make libc6-dev wget bzip2 ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN wget -q https://download.videolan.org/pub/libdvdcss/1.4.3/libdvdcss-1.4.3.tar.bz2 \
    && tar xjf libdvdcss-1.4.3.tar.bz2 \
    && cd libdvdcss-1.4.3 \
    && ./configure --prefix=/usr/local --disable-static \
    && make -j"$(nproc)" \
    && make install

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
    libdvdread8t64 \
    eject \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=dvdcss /usr/local/lib/libdvdcss.so.2* /usr/lib/
COPY --from=builder /build/target/release/bdrip /usr/local/bin/bdrip

ENTRYPOINT ["bdrip"]
