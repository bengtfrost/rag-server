# syntax=docker/dockerfile:1.7

# =============================================================================
# Stage 1: Builder — compile the Rust binary for the target architecture
# =============================================================================
FROM rust:1.100.0-slim-trixie AS builder

# Build dependencies:
#   build-essential  → cc/ld for openssl-sys and the `cc` crate
#   pkg-config       → openssl-sys discovery
#   libssl-dev       → native-tls / openssl-sys
#   curl             → download vec0.so for the target arch
# Note: no libsqlite3-dev — `rusqlite` is built with `features = ["bundled"]`,
# which compiles SQLite from source and statically links it.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        libssl-dev \
        curl \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# -----------------------------------------------------------------------------
# Dependency cache layer
# -----------------------------------------------------------------------------
# Compile a dummy binary first so the dependency graph is built and cached
# in a layer that only invalidates when Cargo.{toml,lock} change.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked

# Real source. .dockerignore keeps target/, .git/, env, and *.db out of the
# build context, so this layer is small.
COPY . .

# -----------------------------------------------------------------------------
# Real build — CRITICAL: cargo clean -p first
# -----------------------------------------------------------------------------
# Do NOT just run `cargo build` here. The `COPY . .` above preserves the
# source files' mtimes from actions/checkout (~T+0s). The dummy binary
# built in the previous layer has an mtime from a few minutes into the
# build (~T+11min). Cargo's freshness check sees the source as *older*
# than the binary and skips rebuild — this layer would `cp
# target/release/rag-server-mcp` and ship the dummy `fn main() {}`
# binary, which silently ignores `--help` and `--version` (exit 0, no
# output).
#
# `cargo clean -p rag-server-mcp` removes only the local crate's binary
# and fingerprint, leaving the dependency cache (`target/release/deps/`,
# `target/release/build/`) intact. The subsequent `cargo build` recompiles
# just the local crate: ~30s native, ~2min under QEMU for arm64.
#
# Regression guard: if the binary at target/release/rag-server-mcp is
# suspiciously small (~300 KB), the fix didn't apply and the dummy
# shipped. The real binary with all deps linked is ~8–10 MB.
RUN cargo clean -p rag-server-mcp \
    && cargo build --release --locked \
    && cp target/release/rag-server-mcp /usr/local/bin/rag-server-mcp \
    && ls -lh /usr/local/bin/rag-server-mcp

# -----------------------------------------------------------------------------
# Download sqlite-vec loadable extension for the target architecture
# -----------------------------------------------------------------------------
# TARGETARCH is injected by BuildKit (amd64 / arm64) and maps to the release
# asset names x86_64 / aarch64. The archive layout isn't guaranteed —
# `find` locates vec0.so wherever it landed.
ARG TARGETARCH
RUN set -eux; \
    case "${TARGETARCH}" in \
        amd64) vec_arch="x86_64"  ;; \
        arm64) vec_arch="aarch64" ;; \
        *) echo "Unsupported TARGETARCH: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/vec0.tar.gz \
        "https://github.com/asg017/sqlite-vec/releases/download/v0.1.9/sqlite-vec-0.1.9-loadable-linux-${vec_arch}.tar.gz"; \
    mkdir -p /tmp/vec0 /out/resources/extensions; \
    tar -xzf /tmp/vec0.tar.gz -C /tmp/vec0; \
    find /tmp/vec0 -name vec0.so -exec cp {} /out/resources/extensions/vec0.so \;; \
    test -f /out/resources/extensions/vec0.so; \
    rm -rf /tmp/vec0 /tmp/vec0.tar.gz; \
    cp /usr/local/bin/rag-server-mcp /out/rag-server-mcp

# =============================================================================
# Stage 2: Runtime — minimal Debian Trixie image
# =============================================================================
FROM debian:trixie-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3t64 \
    && rm -rf /var/lib/apt/lists/*

# Non-root user via `useradd` (from `passwd`, Priority: required — always
# present). `adduser` is NOT in debian:trixie-slim: Debian dropped it from
# the slim base in the Bookworm→Trixie transition.
RUN useradd --create-home --home-dir /home/raguser \
        --shell /usr/sbin/nologin --comment "" raguser

# Layout:
#   /usr/local/bin/rag-server-mcp
#   /usr/local/share/rag-server/resources/tokenizer.json
#   /usr/local/share/rag-server/resources/extensions/vec0.so
COPY --from=builder /out/rag-server-mcp                  /usr/local/bin/rag-server-mcp
COPY resources/tokenizer.json                             /usr/local/share/rag-server/resources/tokenizer.json
COPY --from=builder /out/resources/extensions/vec0.so     /usr/local/share/rag-server/resources/extensions/vec0.so

RUN chmod +x /usr/local/bin/rag-server-mcp \
    && chown -R raguser:raguser /usr/local/share/rag-server

# Since v2.4.2 the binary resolves resources from $RAG_RESOURCES_DIR first.
# Same code path as an installed binary — no special-casing.
ENV RAG_RESOURCES_DIR=/usr/local/share/rag-server/resources

USER raguser
WORKDIR /home/raguser

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/rag-server-mcp"]
CMD ["--help"]
