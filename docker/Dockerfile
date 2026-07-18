# Release image: multi-stage build, distroless-ish runtime. Optimized for
# size and a minimal attack surface, not for build speed (that's what the
# dev image + cached volumes are for).

FROM rust:1-slim AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    clang \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/nonroot --shell /usr/sbin/nologin --uid 10001 nonroot \
    && mkdir -p /data && chown nonroot:nonroot /data

COPY --from=builder /app/target/release/recordagent /usr/local/bin/recordagent

VOLUME /data
ENV RECORDAGENT_STORAGE__PATH=/data \
    RECORDAGENT_SERVER__HOST=0.0.0.0

USER nonroot
EXPOSE 7070

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:7070/healthz || exit 1

ENTRYPOINT ["recordagent"]
CMD ["serve"]
