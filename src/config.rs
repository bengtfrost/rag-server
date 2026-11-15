use anyhow::{Result, bail};
use dirs::config_dir;
use std::env;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

// 4 hours by default — generous because ingest batches hit slow local
// embedding backends. Override with RAG_TIMEOUT_SECS if you know your
// backend is faster; the value is also used as the reqwest client's
// request timeout, so a runaway HTTP call is bounded by this.
const DEFAULT_TIMEOUT_SECS: u64 = 14_400;

const DEFAULT_DB_FILENAME: &str = "vectors.db";
const DEFAULT_TOKENIZER_FILENAME: &str = "tokenizer.json";
/// Subdirectory of the resources dir holding `vec0.{so,dylib,dll}`.
const VEC0_EXTENSION_SUBDIR: &str = "extensions";
/// Sentinel: "don't load a bundled file — let the system dynamic loader
/// resolve `vec0` itself" (for system-installed sqlite-vec).
pub const VEC0_SYSTEM_LOADER: &str = "vec0";

const DEFAULT_EMBED_URL: &str = "http://localhost:11435/v1/embeddings";
const DEFAULT_RERANK_URL: &str = "http://localhost:11436/rerank";
const DEFAULT_EXPAND_URL: &str = "http://localhost:4000/v1/chat/completions";
const DEFAULT_EMBED_MODEL: &str = "bge-m3";
const DEFAULT_RERANK_MODEL: &str = "bge-reranker-v2-m3";

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: String,
    pub tokenizer_path: String,
    /// Path to `vec0.{so,dylib,dll}` (SQLite vector-search extension), or
    /// [`VEC0_SYSTEM_LOADER`] meaning "let the system dynamic loader find it".
    pub vec0_extension_path: String,

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
    /// header.
    pub expand_auth_token: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        // Base directory for defaults. Created best-effort: if it can't
        // be created (read-only FS, sandbox, etc.) we still succeed as
        // long as the user supplied explicit paths via env vars.
        let base_dir =
            config_dir().map_or_else(|| PathBuf::from(".rag-server"), |p| p.join("rag-server"));
        let _ = std::fs::create_dir_all(&base_dir);

        let db_path = env_path("RAG_DB_PATH").unwrap_or_else(|| base_dir.join(DEFAULT_DB_FILENAME));
        let tokenizer_path = resolve_tokenizer_path(&base_dir);
        let vec0_extension_path = resolve_vec0_extension_path();

        let cfg = Self {
            db_path: db_path.to_string_lossy().into_owned(),
            tokenizer_path: tokenizer_path.to_string_lossy().into_owned(),
            vec0_extension_path,

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

        info!(
            tokenizer_path = %cfg.tokenizer_path,
            vec0_extension_path = %cfg.vec0_extension_path,
            "resolved resource paths"
        );

        cfg.validate()?;
        Ok(cfg)
    }

    /// `true` if the vec0 extension is loadable. Returns `true` when we've
    /// fallen back to the system loader (can't `stat` that) and when the
    /// resolved path exists on disk.
    pub fn vec0_extension_available(&self) -> bool {
        if self.vec0_extension_path == VEC0_SYSTEM_LOADER {
            return true;
        }
        Path::new(&self.vec0_extension_path).is_file()
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

        if !self.vec0_extension_available() {
            bail!(
                "vec0 extension not found at '{}'. Since v2.4.2 it is gitignored \
         and must be downloaded into resources/extensions/ once (see \
         README §1), installed sqlite-vec system-wide, or pointed at \
         explicitly via RAG_VEC0_EXTENSION_PATH.",
                self.vec0_extension_path
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
        Ok(s) => {
            if let Ok(v) = s.parse::<T>() {
                v
            } else {
                warn!(
                    var,
                    value = %s,
                    default = %default,
                    "Invalid env value; using default"
                );
                default
            }
        }
        Err(_) => default,
    }
}

/// Parse an optional env var. Missing → `None`. Invalid → `None` + `warn!`.
fn env_parse_opt<T>(var: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    match env::var(var) {
        Ok(s) => {
            if let Ok(v) = s.parse::<T>() {
                Some(v)
            } else {
                warn!(var, value = %s, "Invalid env value; treating as unset");
                None
            }
        }
        Err(_) => None,
    }
}

// ===== resource resolution =====

/// Locate the bundled `resources/` directory. Resolution order:
///   1. `$RAG_RESOURCES_DIR` — explicit override.
///   2. `<dir of current_exe>/resources` — installed binary.
///   3. `<crate root>/resources` — `cargo run` from a checkout.
fn resources_dir() -> Option<PathBuf> {
    if let Some(dir) = env_path("RAG_RESOURCES_DIR") {
        return Some(dir);
    }
    if let Ok(exe) = env::current_exe()
        && let Some(parent) = exe.parent()
    {
        let candidate = parent.join("resources");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    if candidate.is_dir() {
        return Some(candidate);
    }
    None
}

/// Resolve the tokenizer path. Priority:
/// 1. `RAG_TOKENIZER_PATH` — explicit override (used as-is, even if missing).
/// 2. `<resources>/tokenizer.json` — new canonical location.
/// 3. `./tokenizer.json` — pre-move checkout.
/// 4. `<config_dir>/rag-server/tokenizer.json` — legacy installed layout.
fn resolve_tokenizer_path(base_dir: &Path) -> PathBuf {
    if let Some(p) = env_path("RAG_TOKENIZER_PATH") {
        return p;
    }
    if let Some(dir) = resources_dir() {
        let p = dir.join(DEFAULT_TOKENIZER_FILENAME);
        if p.is_file() {
            return p;
        }
    }
    let cwd = PathBuf::from(DEFAULT_TOKENIZER_FILENAME);
    if cwd.is_file() {
        return cwd;
    }
    base_dir.join(DEFAULT_TOKENIZER_FILENAME)
}

/// Resolve the vec0 extension path.
///
/// Public — `db.rs` calls this so it does not have to thread `Config` in.
///
/// Order:
/// 1. `RAG_VEC0_EXTENSION_PATH` — explicit override (new name).
/// 2. `SQLITE_VEC_PATH` — legacy override (the name `db.rs` used).
/// 3. `<resources>/extensions/vec0.{so,dylib,dll}` — canonical bundled location.
/// 4. `./extensions/vec0.{so,dylib,dll}` — legacy CWD-relative checkout.
/// 5. `"vec0"` — let the system dynamic loader find a system-installed copy.
pub fn resolve_vec0_extension_path() -> String {
    for var in ["RAG_VEC0_EXTENSION_PATH", "SQLITE_VEC_PATH"] {
        if let Ok(path) = env::var(var)
            && !path.is_empty()
        {
            return path;
        }
    }
    if let Some(dir) = resources_dir()
        && let Some(path) = find_vec0_in(&dir.join(VEC0_EXTENSION_SUBDIR))
    {
        return path;
    }
    if let Some(path) = find_vec0_in(Path::new(VEC0_EXTENSION_SUBDIR)) {
        return path;
    }
    VEC0_SYSTEM_LOADER.to_string()
}

fn find_vec0_in(dir: &Path) -> Option<String> {
    let p = dir.join(vec0_extension_filename());
    if p.is_file() {
        Some(p.to_string_lossy().into_owned())
    } else {
        None
    }
}

const fn vec0_extension_filename() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "vec0.dylib"
    }
    #[cfg(windows)]
    {
        "vec0.dll"
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        "vec0.so"
    }
}
