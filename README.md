# RAG MCP Server (Rust)

A high‑performance **Rust** implementation of a RAG (Retrieval‑Augmented Generation) server that exposes its functionality via the **Model Context Protocol (MCP)** over stdio. It provides eight tools for managing collections, ingesting documents (from files or raw text), and performing semantic search with automatic query expansion and reranking.

This server is designed to work with any MCP‑compatible client, such as [Claude Desktop](https://claude.ai/download), [Goose](https://github.com/block/goose), or custom applications that speak JSON‑RPC 2.0 over stdin/stdout.

---

## Features

- **Document ingestion** – index files (`.txt`, `.pdf`, `.md`, `.rst`, `.text`) or raw text strings.
- **Semantic search** – using ANN (Approximate Nearest Neighbor) via `sqlite‑vec` with 1024‑dim embeddings.
- **Automatic query expansion** – for broad Swedish queries, using an LLM to generate synonyms/related terms (with a static fallback).
- **Reranking** – reorders search results with a cross‑encoder model for improved relevance.
- **Parallel directory ingestion** – processes multiple files concurrently, respecting a configurable limit (`RAG_MAX_CONCURRENT`).
- **Batch insertion** – `add_documents` now uses a single database transaction for all documents, drastically reducing disk I/O and lock contention.
- **MCP‑compliant** – speaks JSON‑RPC 2.0 over stdio; all logging goes to stderr, leaving stdout clean for the protocol.
- **Fast and safe** – written in Rust for performance and memory safety, using `tokio` for async I/O.

---

## Performance

Recent optimisations ensure efficient operation even with large datasets:

- **Directory ingestion** is throttled using a semaphore to avoid file‑descriptor exhaustion and prevent overwhelming the embedding API.
- **Batch inserts** in `add_documents` group all chunk insertions into a single SQLite transaction, cutting transaction overhead by up to 90% for bulk imports.
- **Concurrent document processing** in `ingest_directory` respects the `RAG_MAX_CONCURRENT` setting (default 4), balancing throughput and system load.

---

## Building

### Prerequisites

- **Rust** 1.79 or later (install via [rustup](https://rustup.rs/)).
- **sqlite‑vec** – the SQLite extension must be installed and loadable. On Fedora:
  ```bash
  sudo dnf install sqlite-vec
  ```
  If the extension is in a non‑standard location, set the path via the `SQLITE_VEC_PATH` environment variable.

### Steps

```bash
git clone https://github.com/bengtfrost/rag-server
cd rag-server
cargo build --release
```

The compiled binary is `target/release/rag-server`.

---

## Configuration

All settings are controlled via environment variables. Default values are shown.

| Variable                | Default                                       | Description                                         |
| ----------------------- | --------------------------------------------- | --------------------------------------------------- |
| `RAG_DB_PATH`           | `~/.local/share/rag-bge-tokeniser/vectors.db` | Path to the SQLite database                         |
| `RAG_EMBED_URL`         | `http://localhost:4000/v1/embeddings`         | Embedding API endpoint                              |
| `RAG_EMBED_MODEL`       | `local-llama-server-embed`                    | Embedding model name                                |
| `RAG_RERANK_URL`        | `http://localhost:4000/rerank`                | Rerank API endpoint                                 |
| `RAG_RERANK_MODEL`      | `local-llama-server-rerank`                   | Rerank model name                                   |
| `RAG_CHUNK_SIZE`        | `512`                                         | Maximum tokens per chunk (word‑based approximation) |
| `RAG_CHUNK_OVERLAP`     | `64`                                          | Overlap tokens between chunks                       |
| `RAG_EMBED_BATCH_SIZE`  | `8`                                           | Batch size for embedding requests                   |
| `RAG_RERANK_CANDIDATES` | `20`                                          | Number of candidates retrieved from ANN search      |
| `RAG_RERANK_MIN_SCORE`  | `0.1`                                         | Minimum relevance score to include in results       |
| `RAG_MAX_CONCURRENT`    | `4`                                           | Maximum concurrent files during directory ingestion |
| `RAG_TIMEOUT`           | `7200`                                        | HTTP timeout in seconds                             |
| `SQLITE_VEC_PATH`       | `sqlite-vec`                                  | Path to the `sqlite‑vec` extension library          |

> **Note:** The tokeniser is a simple word‑based counter, not a full BPE tokeniser. This is a deliberate trade‑off to avoid heavy dependencies and works well for chunk size estimation.

---

## Usage with MCP Clients

The server is a stdio subprocess that reads JSON‑RPC messages from stdin and writes responses to stdout.

### Claude Desktop

Add the following to your `claude_desktop_config.json` (located in `~/.config/Claude/` on Linux):

```json
{
  "mcpServers": {
    "rag-bge": {
      "command": "/absolute/path/to/rag-server",
      "args": []
    }
  }
}
```

### Goose (the AI assistant)

If you use [Goose](https://github.com/block/goose), replace the Python RAG extension with the Rust version in `~/.config/goose/config.yaml`:

```yaml
# ~/.config/goose/config.yaml

GOOSE_TELEMETRY_ENABLED: false
active_provider: openai

providers:
  openai:
    type: openai
    base_url: http://localhost:4000/v1
    api_key: sk-unused

extensions:
  # --- Replace the Python rag extension with the Rust binary ---
  rag:
    enabled: true
    name: rag
    type: stdio
    cmd: /absolute/path/to/rag-server # e.g. /home/bfrost/rag-server/target/release/rag-server
    args: []
    timeout: 14400 # 4 hours – safe for heavy batch indexing

  # Other extensions remain unchanged…
  sqlite:
    enabled: true
    name: sqlite
    type: stdio
    cmd: uvx
    args:
      - mcp-server-sqlite
      - --db-path
      - /home/bfrost/.local/share/rag-bge-tokeniser/vectors.db
    timeout: 300

  fetch:
    enabled: true
    name: fetch
    type: stdio
    cmd: uvx
    args:
      - mcp-server-fetch
    timeout: 300

  codebase-memory:
    enabled: true
    name: codebase-memory
    type: stdio
    cmd: /home/bfrost/.local/bin/codebase-memory-mcp
    env:
      OPENAI_API_BASE: "http://localhost:4000/v1"
      OPENAI_API_KEY: "sk-unused"
      EMBEDDING_MODEL: "local-llama-server-embed"
    timeout: 3600

  developer:
    enabled: true
    type: builtin
    name: developer
  analyze:
    enabled: true
    type: platform
    name: analyze
  skills:
    enabled: true
    type: platform
    name: skills
  todo:
    enabled: true
    type: platform
    name: todo
```

With this configuration, Goose will use the Rust RAG server instead of the Python one, benefiting from lower latency and better concurrency.

---

## Exposed MCP Tools

| Tool                | Description                                                                                                                                      |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `create_collection` | Create a new collection (if it doesn't already exist).                                                                                           |
| `ingest_file`       | Index a single file. Parameters: `collection`, `file_path`, `document_id` (optional), `encoding` (optional), `force` (bool).                     |
| `ingest_directory`  | Index all files in a directory. Parameters: `collection`, `directory_path`, `file_extensions` (optional), `encoding` (optional), `force` (bool). |
| `add_documents`     | Index raw text strings. Parameters: `collection`, `ids` (list), `documents` (list), `force` (bool).                                              |
| `query`             | Perform semantic search. Parameters: `collection`, `query`, `top_k` (optional, default 5).                                                       |
| `list_collections`  | List all collections with document counts.                                                                                                       |
| `delete_documents`  | Remove documents from a collection. Parameters: `collection`, `ids` (list; if empty, the entire collection is cleared).                          |
| `delete_collection` | Delete a whole collection (including all chunks).                                                                                                |

---

## Example JSON‑RPC Calls

### Create a collection

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "create_collection",
    "arguments": { "name": "my_docs" }
  }
}
```

### Ingest a PDF

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "ingest_file",
    "arguments": {
      "collection": "my_docs",
      "file_path": "/home/user/document.pdf",
      "force": false
    }
  }
}
```

### Query

```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "tools/call",
  "params": {
    "name": "query",
    "arguments": {
      "collection": "my_docs",
      "query": "what is the Swedish constitution?"
    }
  }
}
```

---

## Logging

All logs are written to **stderr** to avoid interfering with stdout JSON‑RPC. To view logs when running the server manually, redirect stderr:

```bash
./rag-server 2> rag.log
```

Set the `RUST_LOG` environment variable to control verbosity (e.g., `RUST_LOG=debug`).

---

## Contributing

Issues and pull requests are welcome! Please use the [GitHub repository](https://github.com/bengtfrost/rag-server) for bug reports and feature requests.

---

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.
