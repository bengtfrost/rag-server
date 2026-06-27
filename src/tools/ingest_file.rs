use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;
use std::time::Instant;

use crate::config::Config;
use crate::db::Db;
use crate::extractor::extract_text_from_file;
use crate::chunker::chunk_text_exact;
use crate::embedder::get_embeddings;

#[derive(Debug, Deserialize)]
pub struct IngestFileArgs {
    pub collection: String,
    pub file_path: String,
    #[serde(default)]
    pub document_id: Option<String>,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default)]
    pub force: bool,
}

fn default_encoding() -> String {
    "utf-8".to_string()
}

pub async fn ingest_file(
    db: &Arc<Mutex<Db>>,
    cfg: &Config,
    client: &reqwest::Client,
    args: IngestFileArgs,
) -> anyhow::Result<String> {
    let file_path = &args.file_path;
    if !std::path::Path::new(file_path).is_file() {
        return Err(anyhow::anyhow!("Fil hittades inte: {}", file_path));
    }

    // Use absolute path as fallback to avoid collisions
    let doc_id = args.document_id.clone().unwrap_or_else(|| {
        let abs_path = std::path::absolute(file_path)
            .unwrap_or_else(|_| std::path::Path::new(file_path).to_path_buf());
        abs_path.to_string_lossy().to_lowercase()
    });

    let text = extract_text_from_file(file_path, Some(&args.encoding))?;
    let size_kb = std::fs::metadata(file_path)?.len() / 1024;
    debug!(
        "Startar ingest av '{}' ({} KB, doc_id='{}')",
        std::path::Path::new(file_path).file_name().unwrap_or_default().to_string_lossy(),
        size_kb,
        doc_id
    );

    let db_guard = db.lock().await;
    db_guard.insert_collection(&args.collection)?;

    if db_guard.doc_exists(&args.collection, &doc_id)? && !args.force {
        return Ok(format!(
            "Varning: '{}' är redan indexerat i '{}'. Inga ändringar gjordes.",
            doc_id, args.collection
        ));
    }
    drop(db_guard);

    let t0 = Instant::now();
    let chunks = chunk_text_exact(&text, cfg.chunk_size, cfg.chunk_overlap)?;
    if chunks.is_empty() {
        return Ok("Ingen text att indexera.".to_string());
    }

    let embeddings = get_embeddings(client, cfg, &chunks, &doc_id).await?;
    let db_guard = db.lock().await;
    db_guard.insert_chunks(&args.collection, &doc_id, &chunks, &embeddings)?;
    drop(db_guard);

    let elapsed = t0.elapsed();
    let action = if args.force { "Re-indexerade" } else { "Indexerade" };
    Ok(format!(
        "✓ Klar! {} {} segment från '{}' (doc_id='{}') i '{}' på {}.",
        action,
        chunks.len(),
        std::path::Path::new(file_path).file_name().unwrap_or_default().to_string_lossy(),
        doc_id,
        args.collection,
        format_duration(elapsed)
    ))
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}