# Use the official Rust slim image as the builder stage
FROM rust:1-bullseye AS builder

# Install build dependencies
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       build-essential \
       libsqlite3-dev \
       pkg-config \
       libssl-dev \
       poppler-utils \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /usr/src/rag-server

# Copy manifest files first to leverage build cache for dependencies
COPY Cargo.toml Cargo.lock ./

# Create a dummy src to allow `cargo fetch` to cache dependencies when workspace layout is used
RUN mkdir -p src && echo "fn main() { println!(\"hello\"); }" > src/main.rs || true

# Fetch dependencies (cached unless Cargo.toml/Cargo.lock change)
RUN cargo fetch --locked || true

# Copy the rest of the source
COPY . .

# Build the project in release mode
RUN cargo build --release

# Use a minimal Debian image for the final stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       libsqlite3-0 \
       poppler-utils \
       ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create directories for RAG Server files
RUN mkdir -p /usr/local/lib/rag-server/extensions

# Copy built artifacts from the builder stage (ignore if not present)
COPY --from=builder /usr/src/rag-server/extensions/vec0.so /usr/local/lib/rag-server/extensions/ 2>/dev/null || true
COPY --from=builder /usr/src/rag-server/tokenizer.json /usr/local/lib/rag-server/ 2>/dev/null || true
COPY --from=builder /usr/src/rag-server/target/release/rag-server /usr/local/bin/

# Environment variables
ENV SQLITE_VEC_PATH=/usr/local/lib/rag-server/extensions/vec0.so
ENV RAG_TOKENIZER_PATH=/usr/local/lib/rag-server/tokenizer.json
ENV RAG_DB_PATH=/usr/local/lib/rag-server/vectors.db

# Expose the port used by rag-server (if it listens on 8080; change if different)
EXPOSE 8080

# Set the entrypoint
ENTRYPOINT ["/usr/local/bin/rag-server"]
