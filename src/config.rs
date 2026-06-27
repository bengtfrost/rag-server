use std::env;
use std::path::PathBuf;
use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: String,
    pub embed_url: String,
    pub embed_model: String,
    pub rerank_url: String,
    pub rerank_model: String,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub embed_batch_size: usize,
    pub rerank_candidates: usize,
    pub rerank_min_score: f64,
    pub max_concurrent_files: usize,
    pub timeout_secs: u64,
    pub sqlite_vec_path: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not find home dir"))?;

        let db_path = env::var("RAG_DB_PATH")
            .unwrap_or_else(|_| format!("{}/.local/share/rag-bge-tokeniser/vectors.db", home.display()));
        if let Some(parent) = PathBuf::from(&db_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        Ok(Self {
            db_path,
            embed_url: env::var("RAG_EMBED_URL").unwrap_or_else(|_| "http://localhost:4000/v1/embeddings".to_string()),
            embed_model: env::var("RAG_EMBED_MODEL").unwrap_or_else(|_| "local-llama-server-embed".to_string()),
            rerank_url: env::var("RAG_RERANK_URL").unwrap_or_else(|_| "http://localhost:4000/rerank".to_string()),
            rerank_model: env::var("RAG_RERANK_MODEL").unwrap_or_else(|_| "local-llama-server-rerank".to_string()),
            chunk_size: env::var("RAG_CHUNK_SIZE").unwrap_or_else(|_| "512".to_string()).parse().unwrap_or(512),
            chunk_overlap: env::var("RAG_CHUNK_OVERLAP").unwrap_or_else(|_| "64".to_string()).parse().unwrap_or(64),
            embed_batch_size: env::var("RAG_EMBED_BATCH_SIZE").unwrap_or_else(|_| "8".to_string()).parse().unwrap_or(8),
            rerank_candidates: env::var("RAG_RERANK_CANDIDATES").unwrap_or_else(|_| "20".to_string()).parse().unwrap_or(20),
            rerank_min_score: env::var("RAG_RERANK_MIN_SCORE").unwrap_or_else(|_| "0.1".to_string()).parse().unwrap_or(0.1),
            max_concurrent_files: env::var("RAG_MAX_CONCURRENT").unwrap_or_else(|_| "4".to_string()).parse().unwrap_or(4),
            timeout_secs: env::var("RAG_TIMEOUT").unwrap_or_else(|_| "7200".to_string()).parse().unwrap_or(7200),
            sqlite_vec_path: env::var("SQLITE_VEC_PATH").unwrap_or_else(|_| "sqlite-vec".to_string()),
        })
    }
}