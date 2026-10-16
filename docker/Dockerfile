# Release image: multi-stage build, distroless-ish runtime. Optimized for
# size and a minimal attack surface, not for build speed (that's what the
# dev image + cached volumes are for).

# Both stages pin the *same* Debian release on purpose. `rust:1-slim`
# tracks whatever Debian is current, and when it moved to trixie the
# binary started requiring GLIBC_2.38 / CXXABI_1.3.15 — which
# bookworm-slim does not have. The runtime image then failed to start
# with a linker error, which is a miserable thing to debug from a
# "connection refused" in CI. Bumping one of these means bumping both.
FROM rust:1-slim-trixie AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    clang \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .
RUN cargo build --release

# Bake the embedding model into the image. Without this the first
# `POST /v1/memories` after `docker run` would block on a ~130 MB
# HuggingFace download — and fail outright on an air-gapped host.
#
# `warm-models` also pulls the rerank model when the config it loads
# enables one (reranking-plan.md Phase 5.4). The default build passes
# no `[rerank]` config, so the default image carries no rerank model —
# `mode = off` means no download, no resident weights, no image growth.
# A deployment that wants the second stage baked in passes
# `--build-arg RERANK_MODEL=bge-reranker-base` (the tuned default: ~1.1 GB
# of weights added to the image, and ~1.5 GiB of RSS per `max_concurrency`
# session once serving); with an empty arg the step is exactly what it
# always was. Bake it rather than let the container download it: `/models`
# is an image directory, not a volume, so an unbaked model is re-fetched on
# every container recreate and fails outright with no network.
ARG RERANK_MODEL=
RUN if [ -n "$RERANK_MODEL" ]; then \
      RECUERDOS_AI_EMBEDDINGS__CACHE_DIR=/models \
      RECUERDOS_AI_RERANK__MODE=on \
      RECUERDOS_AI_RERANK__MODEL="$RERANK_MODEL" \
      ./target/release/recuerdos-ai warm-models; \
    else \
      RECUERDOS_AI_EMBEDDINGS__CACHE_DIR=/models \
      ./target/release/recuerdos-ai warm-models; \
    fi

FROM debian:trixie-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    # The ONNX embedding runtime is C++; slim images don't carry it.
    libstdc++6 \
    libgomp1 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/nonroot --shell /usr/sbin/nologin --uid 10001 nonroot \
    && mkdir -p /data && chown nonroot:nonroot /data

COPY --from=builder /app/target/release/recuerdos-ai /usr/local/bin/recuerdos-ai
COPY --from=builder --chown=nonroot:nonroot /models /models

VOLUME /data
ENV RECUERDOS_AI_STORAGE__PATH=/data \
    RECUERDOS_AI_SERVER__HOST=0.0.0.0 \
    RECUERDOS_AI_EMBEDDINGS__CACHE_DIR=/models

USER nonroot
EXPOSE 7070

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:7070/healthz || exit 1

ENTRYPOINT ["recuerdos-ai"]
CMD ["serve"]
