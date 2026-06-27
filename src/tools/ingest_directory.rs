use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use futures::future::join_all;
use tracing::debug;

use crate::config::Config;
use crate::db::Db;
use crate::extractor::extract_text_from_file;
use crate::chunker::chunk_text_exact;
use crate::embedder::get_embeddings;

const DEFAULT_EXTENSIONS: &[&str] = &[".txt", ".pdf", ".md", ".rst", ".text"];

#[derive(Debug, Deserialize)]
pub struct IngestDirectoryArgs {
    pub collection: String,
    pub directory_path: String,
    #[serde(default)]
    pub file_extensions: Option<Vec<String>>,
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[serde(default)]
    pub force: bool,
}

fn default_encoding() -> String {
    "utf-8".to_string()
}

pub async fn ingest_directory(
    db: &Arc<Mutex<Db>>,
    cfg: &Config,
    client: &reqwest::Client,
    args: IngestDirectoryArgs,
) -> anyhow::Result<String> {
    let dir_path = &args.directory_path;
    if !std::path::Path::new(dir_path).is_dir() {
        return Err(anyhow::anyhow!("Katalogen hittades inte: {}", dir_path));
    }

    let exts: Vec<String> = args.file_extensions.clone().unwrap_or_else(|| {
        DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect()
    });
    let exts_set: std::collections::HashSet<String> = exts.iter()
        .map(|e| if e.starts_with('.') { e.clone() } else { format!(".{}", e) })
        .collect();

    let entries = std::fs::read_dir(dir_path)?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                let ext_with_dot = format!(".{}", ext.to_lowercase());
                if exts_set.contains(&ext_with_dot) {
                    files.push(path.to_string_lossy().to_string());
                }
            }
        }
    }
    if files.is_empty() {
        let exts_str = exts_set.iter().cloned().collect::<Vec<_>>().join(", ");
        return Ok(format!(
            "Inga filer med ändelserna [{}] hittades i {}.",
            exts_str, dir_path
        ));
    }

    debug!("Katalogindexering startar: {} filer i '{}' (force={})...", files.len(), dir_path, args.force);
    let db_guard = db.lock().await;
    db_guard.insert_collection(&args.collection)?;
    drop(db_guard);

    let mut prepared = Vec::new();
    for fp in &files {
        let doc_id = std::path::Path::new(fp).file_name().unwrap_or_default().to_string_lossy().to_lowercase();
        match extract_text_from_file(fp, Some(&args.encoding)) {
            Ok(text) => {
                match chunk_text_exact(&text, cfg.chunk_size, cfg.chunk_overlap) {
                    Ok(chunks) => {
                        debug!("  Chunkat: '{}' (doc_id='{}') → {} segment", 
                            std::path::Path::new(fp).file_name().unwrap_or_default().to_string_lossy(), 
                            doc_id, 
                            chunks.len()
                        );
                        prepared.push((fp.clone(), doc_id, chunks, None));
                    }
                    Err(e) => {
                        debug!("  Fel vid chunkning av '{}': {}", 
                            std::path::Path::new(fp).file_name().unwrap_or_default().to_string_lossy(), 
                            e
                        );
                        prepared.push((fp.clone(), doc_id, Vec::new(), Some(e.to_string())));
                    }
                }
            }
            Err(e) => {
                debug!("  Fel vid läsning av '{}': {}", 
                    std::path::Path::new(fp).file_name().unwrap_or_default().to_string_lossy(), 
                    e
                );
                prepared.push((fp.clone(), doc_id, Vec::new(), Some(e.to_string())));
            }
        }
    }

    let sem = Arc::new(Semaphore::new(cfg.max_concurrent_files));
    let mut tasks: Vec<tokio::task::JoinHandle<anyhow::Result<(String, usize, String)>>> = Vec::new();

    for (fp, doc_id, chunks, err) in prepared {
        let db = Arc::clone(db);
        let cfg = cfg.clone();
        let client = client.clone();
        let sem = Arc::clone(&sem);
        let collection = args.collection.clone();
        let force = args.force;
        // Acquire a permit before spawning, limiting concurrency
        let permit = sem.clone().acquire_owned().await.unwrap();
        tasks.push(tokio::spawn(async move {
            let _permit = permit; // held until the task ends
            if let Some(e) = err {
                return Ok((std::path::Path::new(&fp).file_name().unwrap_or_default().to_string_lossy().to_string(), 0, format!("fel – {}", e)));
            }
            if chunks.is_empty() {
                return Ok((std::path::Path::new(&fp).file_name().unwrap_or_default().to_string_lossy().to_string(), 0, "tom fil".to_string()));
            }
            let db_guard = db.lock().await;
            if db_guard.doc_exists(&collection, &doc_id)? && !force {
                return Ok((std::path::Path::new(&fp).file_name().unwrap_or_default().to_string_lossy().to_string(), 0, "redan indexerad".to_string()));
            }
            drop(db_guard);

            let embeddings = get_embeddings(&client, &cfg, &chunks, &doc_id).await?;
            let db_guard = db.lock().await;
            db_guard.insert_chunks(&collection, &doc_id, &chunks, &embeddings)?;
            drop(db_guard);

            let action = if force { "re-indexerad" } else { "indexerad" };
            Ok((std::path::Path::new(&fp).file_name().unwrap_or_default().to_string_lossy().to_string(), chunks.len(), format!("{} segment {}", chunks.len(), action)))
        }));
    }

    let results = join_all(tasks).await;
    let mut result_lines = Vec::new();
    let mut total_segments = 0;
    for res in results {
        match res {
            Ok(Ok((filename, seg, msg))) => {
                total_segments += seg;
                let prefix = if seg > 0 { "✓" } else if msg.contains("redan indexerad") || msg.contains("hoppades") { "⚠" } else { "✗" };
                result_lines.push(format!("  {} {}: {}", prefix, filename, msg));
            }
            Ok(Err(e)) => {
                result_lines.push(format!("  ✗ {}: {}", "fil", e));
            }
            Err(e) => {
                result_lines.push(format!("  ✗ task panik: {}", e));
            }
        }
    }

    let summary = format!(
        "Katalogindexering klar. {} filer · {} segment · samling: '{}'",
        files.len(), total_segments, args.collection
    );
    debug!("{}", summary);
    Ok(summary + "\n" + &result_lines.join("\n"))
}