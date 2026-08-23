# Browser worker image: Debian + Chromium + the compiled worker binary.
# Built from the workspace root: docker build -f deploy/browser-worker/Dockerfile .
FROM rust:1.97-slim AS build
WORKDIR /build
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY services services
RUN cargo build --release --locked --bin ratatoskr-browser-worker

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends chromium ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/target/release/ratatoskr-browser-worker /usr/local/bin/
ENV CHROME_BIN=/usr/bin/chromium
ENTRYPOINT ["ratatoskr-browser-worker"]
