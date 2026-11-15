# syntax=docker/dockerfile:1.7

# =============================================================================
# Stage 1: Builder — compile the Rust binary for the target architecture
# =============================================================================
# Pinned to the patch-level tag (1.100.0) rather than the major-minor (1.100),
# matching a known-good pin from a 2026-11-13 GitLab Containerfile.
# Fallbacks if this tag ever 404s:
#   rust:1.100-slim-trixie
#   rust:1.99-slim-trixie
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

# Cache dependencies: build a dummy binary first so the dependency graph is
# compiled and cached in a layer that only invalidates when Cargo.{toml,lock}
# change. The real `cargo build` below recompiles only the local crate.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

# Real source. .dockerignore keeps target/, .git/, env, and *.db out of the
# build context, so this layer is small.
COPY . .

# Build the actual binary.
RUN cargo build --release --locked \
    && cp target/release/rag-server-mcp /usr/local/bin/rag-server-mcp

# Download the sqlite-vec loadable extension for the *target* architecture.
# TARGETARCH is injected by BuildKit (amd64 / arm64) and maps to the release
# asset names x86_64 / aarch64.
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
    # Archive layout isn't guaranteed — locate vec0.so wherever it landed.
    find /tmp/vec0 -name vec0.so -exec cp {} /out/resources/extensions/vec0.so \;; \
    test -f /out/resources/extensions/vec0.so; \
    rm -rf /tmp/vec0 /tmp/vec0.tar.gz; \
    cp /usr/local/bin/rag-server-mcp /out/rag-server-mcp

# =============================================================================
# Stage 2: Runtime — minimal Debian Trixie image
# =============================================================================
# Trixie is current stable (Bookworm is oldstable since 2025-08).
# OpenSSL runtime package is libssl3t64 after the t64 transition.
FROM debian:trixie-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3t64 \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged user. /home/raguser is writable, so the default DB path
# ($XDG_CONFIG_HOME/rag-server/vectors.db) works.
RUN adduser --disabled-password --gecos "" raguser

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
