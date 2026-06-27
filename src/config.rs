use anyhow::Result;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    #[allow(dead_code)]
    pub base_dir: PathBuf,
    pub db_path: String,
    pub tokenizer_path: String,
    pub embed_url: String,
    pub rerank_url: String,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub embed_batch_size: usize,
    pub rerank_candidates: usize,
    pub timeout_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let base_dir = PathBuf::from("/home/bfrost/.config/rag-server");
        std::fs::create_dir_all(&base_dir)?;

        Ok(Self {
            db_path: base_dir.join("vectors.db").to_string_lossy().into_owned(),
            tokenizer_path: base_dir
                .join("tokenizer.json")
                .to_string_lossy()
                .into_owned(),
            embed_url: env::var("RAG_EMBED_URL")
                .unwrap_or_else(|_| "http://localhost:11435/v1/embeddings".to_string()),
            rerank_url: env::var("RAG_RERANK_URL")
                .unwrap_or_else(|_| "http://localhost:11436/rerank".to_string()),
            chunk_size: env::var("RAG_CHUNK_SIZE")
                .unwrap_or_else(|_| "1024".to_string())
                .parse()
                .unwrap_or(1024),
            chunk_overlap: env::var("RAG_CHUNK_OVERLAP")
                .unwrap_or_else(|_| "150".to_string())
                .parse()
                .unwrap_or(150),
            embed_batch_size: env::var("RAG_BATCH_SIZE")
                .unwrap_or_else(|_| "8".to_string())
                .parse()
                .unwrap_or(8),
            rerank_candidates: env::var("RAG_RERANK_K")
                .unwrap_or_else(|_| "15".to_string())
                .parse()
                .unwrap_or(15),
            timeout_secs: 14400, // 4 timmar som standard för tunga jobb
            base_dir,
        })
    }
}
