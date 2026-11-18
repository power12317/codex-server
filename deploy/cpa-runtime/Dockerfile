# syntax=docker/dockerfile:1
# Override both image arguments with immutable digests for a deployment build.
ARG RUST_IMAGE=rust:1.99.1-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim
FROM ${RUST_IMAGE} AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 clang cmake pkg-config libssl-dev libasound2-dev libudev-dev libcap-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
WORKDIR /src/codex-rs
ENV CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_BUILD_JOBS=2
# Synchronize the workspace and lockfile in this disposable build copy. Stable
# source tags may already carry the release version in the workspace manifest.
RUN python3 ../deploy/cpa-runtime/prepare-release.py
# Codex Server uses the native request builder, OAuth library and streaming
# bridge. It does not execute tools or need the CLI, V8 host or sandbox helpers.
RUN cargo build --locked --release -p codex-app-server --bin codex-app-server
RUN mkdir /out && cp target/release/codex-app-server /out/

FROM ${RUNTIME_IMAGE}
ARG FORK_REVISION=unrecorded-local-build
LABEL org.opencontainers.image.title="Codex Server" \
    org.opencontainers.image.description="Native Codex inference service with credential workers and a streaming API" \
    org.opencontainers.image.revision=${FORK_REVISION} \
    org.opencontainers.image.source="https://github.com/power12317/codex-server" \
    io.codex.cpa.execution-mode="inference-only"
# Keep /etc/localtime a regular bind-mount target. Following Debian's default
# symlink would mount host data over the image's Etc/UTC zoneinfo file instead.
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    tzdata ca-certificates libssl3 libasound2 libudev1 libcap2 git ripgrep bash \
    && rm -rf /var/lib/apt/lists/* \
    && cp /etc/localtime /tmp/codex-localtime \
    && rm /etc/localtime \
    && mv /tmp/codex-localtime /etc/localtime \
    && useradd --uid 10001 --create-home codex \
    && install -d -o codex -g codex -m 0700 /var/lib/codex /run/codex
COPY --from=build /out/ /usr/local/bin/
COPY deploy/cpa-runtime/entrypoint.sh /usr/local/bin/codex-cpa-entrypoint
ENV CODEX_HOME=/var/lib/codex CODEX_CPA_PORT=38317 CODEX_CPA_AUTH_DIR=/cpa-auth
USER codex
WORKDIR /home/codex
ENTRYPOINT ["/usr/local/bin/codex-cpa-entrypoint"]
