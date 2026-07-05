# Use Fedora 44 as the base image (closest to Fedora 44)
FROM fedora:44 AS builder

# Install Rust and dependencies
RUN dnf install -y \
    rust \
    cargo \
    sqlite-devel \
    poppler-utils \
    && dnf clean all

# Set the working directory
WORKDIR /usr/src/rag-server

# Copy the source code
COPY . .

# Build the project in release mode
RUN cargo build --release

# Use a minimal Fedora image for the final stage
FROM fedora:44

# Install runtime dependencies
RUN dnf install -y \
    sqlite \
    poppler-utils \
    && dnf clean all

# Create directories for RAG Server files
RUN mkdir -p /usr/local/lib/rag-server/extensions

# Copy the vec0.so extension
COPY --from=builder /usr/src/rag-server/extensions/vec0.so /usr/local/lib/rag-server/extensions/

# Copy the tokenizer.json file
COPY --from=builder /usr/src/rag-server/tokenizer.json /usr/local/lib/rag-server/

# Copy the binary from the builder stage
COPY --from=builder /usr/src/rag-server/target/release/rag-server /usr/local/bin/

# Set environment variables
ENV SQLITE_VEC_PATH=/usr/local/lib/rag-server/extensions/vec0.so
ENV RAG_TOKENIZER_PATH=/usr/local/lib/rag-server/tokenizer.json
ENV RAG_DB_PATH=/usr/local/lib/rag-server/vectors.db

# Set the entrypoint
ENTRYPOINT ["rag-server"]
