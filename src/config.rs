use anyhow::{Result, bail};
use dirs::config_dir;
use std::env;
use std::path::{Path, PathBuf};
use tracing::warn;

// 4 hours by default — generous because ingest batches hit slow local
// embedding backends. Override with RAG_TIMEOUT_SECS if you know your
// backend is faster; the value is also used as the reqwest client's
// request timeout, so a runaway HTTP call is bounded by this.
const DEFAULT_TIMEOUT_SECS: u64 = 14_400;

const DEFAULT_DB_FILENAME: &str = "vectors.db";
const DEFAULT_TOKENIZER_FILENAME: &str = "tokenizer.json";

const DEFAULT_EMBED_URL: &str = "http://localhost:11435/v1/embeddings";
const DEFAULT_RERANK_URL: &str = "http://localhost:11436/rerank";
const DEFAULT_EXPAND_URL: &str = "http://localhost:4000/v1/chat/completions";
const DEFAULT_EMBED_MODEL: &str = "bge-m3";
const DEFAULT_RERANK_MODEL: &str = "bge-reranker-v2-m3";

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: String,
    pub tokenizer_path: String,

    pub embed_url: String,
    pub rerank_url: String,
    /// OpenAI-compatible chat-completions endpoint used by `expander.rs`.
    /// Empty token (see `expand_auth_token`) means no auth header is sent.
    pub expand_url: String,

    pub embed_model: String,
    pub rerank_model: String,

    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub embed_batch_size: usize,
    pub rerank_candidates: usize,

    pub timeout_secs: u64,
    pub rerank_min_score: f64,
    pub max_concurrent_files: usize,

    pub max_concurrent_requests: Option<usize>,
    pub rerank_min_candidates: Option<usize>,

    /// Bearer token sent to `expand_url`. `None` = no `Authorization`
    /// header. (Replace the hardcoded `"Bearer sk-unused"` in
    /// `expander.rs` with this — see crate notes.)
    pub expand_auth_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Base directory for defaults. Created best-effort: if it can't
        // be created (read-only FS, sandbox, etc.) we still succeed as
        // long as the user supplied explicit paths via env vars.
        let base_dir = config_dir()
            .map(|p| p.join("rag-server"))
            .unwrap_or_else(|| PathBuf::from(".rag-server"));
        let _ = std::fs::create_dir_all(&base_dir);

        let db_path = env_path("RAG_DB_PATH").unwrap_or_else(|| base_dir.join(DEFAULT_DB_FILENAME));
        let tokenizer_path = resolve_tokenizer_path(&base_dir);

        let cfg = Self {
            db_path: db_path.to_string_lossy().into_owned(),
            tokenizer_path: tokenizer_path.to_string_lossy().into_owned(),

            embed_url: env_string("RAG_EMBED_URL", DEFAULT_EMBED_URL),
            rerank_url: env_string("RAG_RERANK_URL", DEFAULT_RERANK_URL),
            expand_url: env_string("RAG_EXPAND_URL", DEFAULT_EXPAND_URL),

            embed_model: env_string("RAG_EMBED_MODEL", DEFAULT_EMBED_MODEL),
            rerank_model: env_string("RAG_RERANK_MODEL", DEFAULT_RERANK_MODEL),

            chunk_size: env_parse("RAG_CHUNK_SIZE", 1024),
            chunk_overlap: env_parse("RAG_CHUNK_OVERLAP", 150),
            embed_batch_size: env_parse("RAG_BATCH_SIZE", 8),
            rerank_candidates: env_parse("RAG_RERANK_K", 15),

            timeout_secs: env_parse("RAG_TIMEOUT_SECS", DEFAULT_TIMEOUT_SECS),
            rerank_min_score: env_parse("RAG_RERANK_MIN_SCORE", 0.3),
            max_concurrent_files: env_parse("RAG_MAX_CONCURRENT_FILES", 4),

            max_concurrent_requests: env_parse_opt("RAG_MAX_CONCURRENT_REQUESTS"),
            rerank_min_candidates: env_parse_opt("RAG_RERANK_MIN_CANDIDATES"),
            expand_auth_token: env_nonempty("RAG_EXPAND_AUTH_TOKEN"),
        };

        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        // These two panic at runtime if left at 0, so hard-fail early.
        if self.chunk_size == 0 {
            bail!("RAG_CHUNK_SIZE must be > 0 (got 0)");
        }
        if self.embed_batch_size == 0 {
            bail!("RAG_BATCH_SIZE must be > 0 (got 0)");
        }
        if self.timeout_secs == 0 {
            bail!("RAG_TIMEOUT_SECS must be > 0 (got 0)");
        }

        // Valid but usually a mistake; the chunker still works, it just
        // produces degenerate overlap. Warn loudly, don't fail.
        if self.chunk_overlap >= self.chunk_size {
            warn!(
                chunk_size = self.chunk_size,
                chunk_overlap = self.chunk_overlap,
                "chunk_overlap >= chunk_size — overlap will not fit in a chunk; \
                 expect repeated/empty chunks. Lower RAG_CHUNK_OVERLAP."
            );
        }

        Ok(())
    }
}

// ===== env helpers =====

fn env_string(var: &str, default: &str) -> String {
    env::var(var).unwrap_or_else(|_| default.to_string())
}

fn env_nonempty(var: &str) -> Option<String> {
    env::var(var).ok().filter(|s| !s.is_empty())
}

fn env_path(var: &str) -> Option<PathBuf> {
    env::var(var)
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Parse an env var, falling back to `default` if missing *or* invalid.
/// Invalid values emit a `warn!` so silent fallbacks are impossible.
fn env_parse<T>(var: &str, default: T) -> T
where
    T: std::str::FromStr + std::fmt::Display + Copy,
{
    match env::var(var) {
        Ok(s) => match s.parse::<T>() {
            Ok(v) => v,
            Err(_) => {
                warn!(
                    var,
                    value = %s,
                    default = %default,
                    "Invalid env value; using default"
                );
                default
            }
        },
        Err(_) => default,
    }
}

/// Parse an optional env var. Missing → `None`. Invalid → `None` + `warn!`.
fn env_parse_opt<T>(var: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    match env::var(var) {
        Ok(s) => match s.parse::<T>() {
            Ok(v) => Some(v),
            Err(_) => {
                warn!(var, value = %s, "Invalid env value; treating as unset");
                None
            }
        },
        Err(_) => None,
    }
}

/// Resolve the tokenizer path.
///
/// Priority:
/// 1. `RAG_TOKENIZER_PATH` (explicit override).
/// 2. `./tokenizer.json` if it exists — matches a fresh clone of the repo,
///    where the tokenizer ships alongside the source.
/// 3. `<config_dir>/rag-server/tokenizer.json` — the installed-layout default.
fn resolve_tokenizer_path(base_dir: &Path) -> PathBuf {
    if let Some(p) = env_path("RAG_TOKENIZER_PATH") {
        return p;
    }
    let cwd_candidate = PathBuf::from(DEFAULT_TOKENIZER_FILENAME);
    if cwd_candidate.is_file() {
        return cwd_candidate;
    }
    base_dir.join(DEFAULT_TOKENIZER_FILENAME)
}
