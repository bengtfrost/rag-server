# =============================================================================
# Stage 1: Builder – compile the Rust binary
# =============================================================================
FROM debian:bookworm-slim AS builder

# Install build dependencies (OpenSSL 3, SQLite, etc.)
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    pkg-config \
    libsqlite3-dev \
    libssl-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install Rust (latest stable)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Set up the build environment
WORKDIR /app

# Cache dependencies – copy only Cargo files first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release \
    && rm -rf src target/release/deps/rag_server*

# Copy the actual source code
COPY . .

# Build the release binary (dependencies are already cached)
RUN cargo build --release

# =============================================================================
# Stage 2: Runtime – minimal Debian Bookworm image
# =============================================================================
FROM debian:bookworm-slim AS runtime

# Install runtime dependencies (OpenSSL 3, SQLite, CA certificates)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libsqlite3-0 \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create a non‑root user for security
RUN adduser --disabled-password --gecos "" raguser

# Copy the built binary from the builder stage
# The package name in Cargo.toml is `rag-server-mcp`, so the compiled binary
# will be `rag-server-mcp`. Copy and install it as `/usr/local/bin/rag-server`.
COPY --from=builder /app/target/release/rag-server-mcp /usr/local/bin/rag-server

# Make sure the binary is executable
RUN chmod +x /usr/local/bin/rag-server

# Switch to non‑root user
USER raguser

# Set the entrypoint
ENTRYPOINT ["/usr/local/bin/rag-server"]

# Default command – help (shows usage)
CMD ["--help"]
