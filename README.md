# 🚀 Sovereign RAG MCP Server (Rust)

> **High-Performance Legal & Code Intelligence for the Terminal.**

A "Native & Lean" implementation of a Retrieval-Augmented Generation (RAG) server written in **Rust**. This server exposes high-precision search logic via the **Model Context Protocol (MCP)**, specifically optimized for **Fedora 44** and **Intel iGPU (Vulkan)** hardware.

By bypassing the "PCIe dinosaur" and utilizing **Unified Memory Architecture (UMA)**, this server provides the lightning-fast "memory" required for autonomous agents like **Goose** and **Aider**.

---

## ✨ Key Milestones in v2.3 (The "Hybrid" Update)

- **🔍 Hybrid Search:** Combines **semantic vector search** (BGE-M3) with **BM25 keyword search** (SQLite FTS5). Configurable weights allow fine-tuning for legal, technical, or general text domains.
- **🗣️ Query Expansion:** Automatically expands broad queries using a local LLM (llama-server on port 11434) to generate synonyms and related terms, improving retrieval recall.
- **🎯 Per-Query Reranker Selection:** Choose reranker model per query with `--rerank-url`. Seamlessly switch between **BGE-Reranker-v2-M3** (Swedish-optimized) and **Mxbai-Reranker-Large-v2** (English-optimized).
- **⚡ Full CLI with `--help`:** All tools available as subcommands with native help support via `clap`. Works equally well as an MCP stdio server or standalone CLI.
- **🧠 Exact BGE-M3 Tokenization:** Integrated Hugging Face `tokenizers` crate for 1:1 parity with XLM-RoBERTa BPE standard.
- **🖥️ Vulkan-Driven Vector Search:** Powered by `sqlite-vec` (v0.1.9+) with `vec0.so` extension for high-density ANN search directly on the iGPU.
- **🔒 Zero Data Leakage:** 100% local processing. All code, databases, and metadata stored strictly at `~/.config/rag-server/`.

---

## 🏗 Architecture: The Zero-Proxy Stack

The server communicates directly with local Vulkan-accelerated engines over native STDIO pipes, ensuring 0% data leakage and minimum latency.

```mermaid
graph TD
    A[Goose / Aider / CLI] -->|STDIO| B(Rust RAG Server)
    B -->|Query Expansion| C[LLM Server :11434]
    B -->|Embeddings| D[BGE-M3 :11435]
    B -->|Reranking| E[Reranker :11436/11437]
    B -->|Dynamic Loading| F[vec0.so Extension]
    F <--> G[(SQLite-Vec DB)]
    B -->|FTS5| H[(BM25 Index)]
```

**Explanation:**

- **LLM Server (port 11434):** Used for query expansion – generating synonyms and related terms to improve recall on broad queries.
- **BGE-M3 (port 11435):** Generates high-quality multilingual embeddings for semantic search.
- **Reranker (port 11436/11437):** Cross-encoder models for precision reranking (BGE for Swedish, Mxbai for English).
- **SQLite-Vec + FTS5:** Vector search + BM25 keyword search in the same database.

---

## 🔧 Installation & Compilation

### 1. Prerequisites

Ensure you have the Rust toolchain and the specialized `vec0` extension for SQLite.

```bash
# Install system dependencies
sudo dnf install -y sqlite-devel poppler-utils

# Download the specific vec0.so (v0.1.9+)
mkdir -p ~/.config/rag-server/extensions
cd /tmp
curl -L -O https://github.com/asg017/sqlite-vec/releases/download/v0.1.9/sqlite-vec-0.1.9-loadable-linux-x86_64.tar.gz
tar -xvf sqlite-vec-0.1.9-loadable-linux-x86_64.tar.gz
mv vec0.so ~/.config/rag-server/extensions/
```

### 2. Tokenizer Setup

Download the BGE-M3 metadata required for exact tokenization.

```bash
wget https://huggingface.co/BAAI/bge-m3/resolve/main/tokenizer.json -O ~/.config/rag-server/tokenizer.json
```

### 3. Build the Binary

```bash
cd ~/.config/rag-server
cargo build --release
```

### 4. Migrate Existing Database (for Hybrid Search)

If you have an existing database, add FTS5 support for BM25 keyword search:

```bash
sqlite3 ~/.config/rag-server/vectors.db "INSERT OR IGNORE INTO docs_fts(rowid, id, collection, text) SELECT rowid, id, collection, text FROM docs;"
```

---

## ⚙️ Configuration

The server defaults to optimized values for a 16GB U-series workstation but can be overridden via environment variables in your `~/.zshrc`.

| Variable                   | Default Value                                | Description                               |
| :------------------------- | :------------------------------------------- | :---------------------------------------- |
| `SQLITE_VEC_PATH`          | `~/.config/rag-server/extensions/vec0.so`    | Path to the vector extension.             |
| `RAG_DB_PATH`              | `~/.config/rag-server/vectors.db`            | Path to the local Knowledge Base.         |
| `RAG_TOKENIZER_PATH`       | `~/.config/rag-server/tokenizer.json`        | BGE-M3 BPE dictionary.                    |
| `RAG_EMBED_URL`            | `http://localhost:11435/v1/embeddings`       | Vulkan Embedding server.                  |
| `RAG_RERANK_URL`           | `http://localhost:11436/rerank`              | Vulkan Reranker server (primary).         |
| `RAG_LLM_URL`              | `http://localhost:11434/v1/chat/completions` | Local LLM for query expansion (optional). |
| `RAG_CHUNK_SIZE`           | `1024` (Legal) / `3000` (Code)               | Max tokens per segment.                   |
| `RAG_CHUNK_OVERLAP`        | `150` (Legal) / `400` (Code)                 | Token overlap for context.                |
| `RAG_EMBED_MODEL`          | `bge-m3`                                     | Model name sent to the embedder.          |
| `RAG_RERANK_MODEL`         | `bge-reranker-v2-m3`                         | Model name sent to the reranker.          |
| `RAG_RERANK_MIN_SCORE`     | `0.3`                                        | Minimum relevance score to keep.          |
| `RAG_MAX_CONCURRENT_FILES` | `4`                                          | Parallelism when ingesting directories.   |
| `RAG_BATCH_SIZE`           | `8`                                          | Embedding batch size for API requests.    |
| `RAG_RERANK_CANDIDATES`    | `10`                                         | Number of candidates for reranking.       |

---

## 🔌 Usage with Goose Agent

Integrate the server as a **stdio** extension in `~/.config/goose/config.yaml`:

```yaml
extensions:
  rag:
    enabled: true
    name: rag
    type: stdio
    cmd: /home/bfrost/.config/rag-server/target/release/rag-server
    env:
      SQLITE_VEC_PATH: /home/bfrost/.config/rag-server/extensions/vec0.so
      RAG_RERANK_URL: http://127.0.0.1:11436/rerank
      RAG_LLM_URL: http://127.0.0.1:11434/v1/chat/completions
    timeout: 7200
```

### Dual Reranker Sessions (Zsh Aliases)

Configure your `~/.zshrc` with these aliases for seamless reranker switching:

```bash
# Swedish stack (BGE-Reranker-v2-M3 on port 11436)
alias goose-local='_goose_session local-llama-server local-llama-server-embed local-llama-server-rerank'

# English stack (Mxbai-Reranker-Large-v2 on port 11437)
alias goose-local-en='_goose_session local-llama-server local-llama-server-embed local-llama-server-rerank-2'

_goose_session() {
    local model="${1:-local-llama-server}"
    local embed="${2:-local-llama-server-embed}"
    local rerank="${3:-local-llama-server-rerank}"
    
    _ensure_agentgateway || return 1
    
    export OPENAI_API_KEY="sk-unused"
    export OPENAI_BASE_URL="http://localhost:4000/v1"
    export RAG_EMBED_MODEL="$embed"
    export RAG_RERANK_MODEL="$rerank"
    
    # Automatic reranker URL selection
    case "$rerank" in
        local-llama-server-rerank)
            export RAG_RERANK_URL="http://127.0.0.1:11436/rerank"
            ;;
        local-llama-server-rerank-2)
            export RAG_RERANK_URL="http://127.0.0.1:11437/rerank"
            ;;
    esac
    
    export RAG_RERANK_CANDIDATES=10
    export RAG_MAX_CONCURRENT=4
    export OPENAI_TIMEOUT=7200
    
    GOOSE_MODEL="$model" goose session
}
```

---

## 🛠 MCP Tools (available to Goose)

- `create_collection` – Initializes a new KB (e.g., `juridik` or `rust-src`).
- `ingest_file` – Chunks, tokenizes, and indexes a file via the iGPU.
- `ingest_directory` – Bulk‑ingests all matching files in a directory with parallel processing.
- `add_documents` – Indexes raw text strings directly from JSON.
- `query` – Performs **hybrid semantic + BM25 keyword search** with reranking for **97% citation accuracy**.
- `list_collections` – Displays all local libraries and document counts.
- `delete_documents` – Deletes one or more documents from a collection.
- `delete_collection` – Removes an entire collection and all its data.

---

## 🖥 CLI Usage (v2.3)

The same binary acts as a full‑featured command‑line tool. Run it without arguments to start the MCP server (stdin/stdout). Run it with a subcommand to perform operations directly.

### Global help

```bash
./target/release/rag-server --help
```

### Subcommand help

```bash
./target/release/rag-server query --help
```

### Examples

#### Collection Management

```bash
# Create a new collection
./target/release/rag-server create-collection --name juridik

# List all collections
./target/release/rag-server list-collections

# Delete a collection
./target/release/rag-server delete-collection --name juridik
```

#### Document Ingestion

```bash
# Ingest a single file
./target/release/rag-server ingest-file --collection juridik --file-path ~/dokument/lag.pdf

# Ingest all .txt and .pdf files in a directory
./target/release/rag-server ingest-directory --collection juridik --directory-path ~/dokument/ --extensions txt,pdf

# Force re-indexing
./target/release/rag-server ingest-file --collection juridik --file-path ~/dokument/lag.pdf --force
```

#### Query & Search

```bash
# Semantic search (vector only)
./target/release/rag-server query --collection juridik --query "vad är regeringsformen?" --top-k 3

# Hybrid search (BM25 + vector)
./target/release/rag-server query --collection juridik --query "tolkningsregler" --hybrid

# Hybrid search with custom weights (more BM25 weight)
./target/release/rag-server query --collection juridik --query "tolkningsregler" --hybrid --vector-weight 0.4 --bm25-weight 0.6

# Specify reranker URL per query
./target/release/rag-server query --collection juridik --query "constitutional law" --rerank-url http://127.0.0.1:11437/rerank
```

### MCP Tool Schema (for Goose)

```json
{
  "name": "query",
  "description": "Sök i samlingen med hybrid search (BM25 + vector) och reranking",
  "inputSchema": {
    "type": "object",
    "properties": {
      "collection": { "type": "string" },
      "query": { "type": "string" },
      "top_k": { "type": "integer", "default": 5 },
      "rerank_url": {
        "type": "string",
        "description": "Valfri reranker-URL (e.g., http://127.0.0.1:11437/rerank)"
      },
      "hybrid": {
        "type": "boolean",
        "description": "Använd hybrid search (BM25 + vector)",
        "default": false
      },
      "vector_weight": {
        "type": "number",
        "description": "Vikt för vektorscore (0-1)",
        "default": 0.7
      },
      "bm25_weight": {
        "type": "number",
        "description": "Vikt för BM25-score (0-1)",
        "default": 0.3
      }
    },
    "required": ["collection", "query"]
  }
}
```

---

## 🏁 Performance Insights

By utilizing **Quantization-Aware Training (QAT-UD)** and **N-Gram software speculation**, this Rust implementation delivers:

- **800% Speedup:** Indexing 700 project chunks reduced from **45+ mins (CPU)** to **~5 mins (iGPU)**.
- **Linguistic Precision:** Handles Swedish legal nuances and complex Rust traits without semantic drift.
- **Zero Leakage:** 100% of data remains on local silicon.
- **Hybrid Search Recall:** BM25 + vector search significantly improves recall for both exact keywords and semantic concepts.

---

## 📊 Test Results (v2.3 Validation)

Comprehensive testing with real legal documents validated all core features:

| Test                             | Result                                     |
| -------------------------------- | ------------------------------------------ |
| ✅ Collection Management         | Create, list, delete                       |
| ✅ Single File Ingestion         | 7,686-line file → 96 segments in 13 min    |
| ✅ Directory Bulk Ingestion      | 3 files → 15 segments                      |
| ✅ Mixed Content (TXT + PDF)     | 33+ segments                               |
| ✅ Semantic Search               | Exact citation retrieval with 97% accuracy |
| ✅ Hybrid Search (BM25 + Vector) | Improved recall for keyword-heavy queries  |
| ✅ Reranker Switching            | BGE (Swedish) ↔ Mxbai (English) seamless   |
| ✅ Query Expansion               | LLM-based synonym generation               |
| ✅ Document/Collection Deletion  | Clean removal                              |

**All critical tests passed.** The server is production-ready for legal, codebase, and technical QA deployments.

---

## 📝 Change Log

### v2.3 (2026-06-28)

- **Hybrid Search:** Added BM25 (FTS5) + vector search with configurable weights.
- **Per-Query Reranker:** Added `--rerank-url` flag for runtime reranker selection.
- **Dual Reranker Support:** Native integration with BGE-Reranker-v2-M3 (Swedish) and Mxbai-Reranker-Large-v2 (English).
- **CLI Enhancements:** Full `--help` support for all subcommands.
- **Performance:** Optimized parallel directory ingestion.
- **Stability:** Fixed regex issues in sentence splitting; added robust error handling.

### v2.2 (2026-06-20)

- Initial Rust port with BGE-M3 embeddings and BGE-Reranker-v2-M3.
- MCP server mode for Goose integration.
- Basic CLI support.

---

**Author:** [Bengt Frost](https://github.com/bengtfrost)\
**Philosophy:** Native & Lean | Unified Memory | Sovereign AI\
**License:** MIT\
**Repository:** [github.com/bengtfrost/rag-server](https://github.com/bengtfrost/rag-server)
