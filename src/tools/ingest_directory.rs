use clap::Args;
use futures::future::join_all;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tracing::debug;

use crate::chunker::chunk_text_exact;
use crate::config::Config;
use crate::db::Db;
use crate::embedder::get_embeddings;
use crate::extractor::extract_text_from_file;

const DEFAULT_EXTENSIONS: &[&str] = &[".txt", ".pdf", ".md", ".rst", ".text"];

#[derive(Debug, Deserialize, Args)]
pub struct IngestDirectoryArgs {
    #[arg(short, long)]
    pub collection: String,
    #[arg(short, long)]
    pub directory_path: String,
    #[arg(short = 'x', long = "extensions", value_delimiter = ',')]
    #[serde(default)]
    pub file_extensions: Option<Vec<String>>,
    #[arg(short, long, default_value = "utf-8")]
    #[serde(default = "default_encoding")]
    pub encoding: String,
    #[arg(short, long)]
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
        return Err(anyhow::anyhow!("Directory not found: {}", dir_path));
    }

    let exts: Vec<String> = args
        .file_extensions
        .clone()
        .unwrap_or_else(|| DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect());
    let exts_set: std::collections::HashSet<String> = exts
        .iter()
        .map(|e| {
            if e.starts_with('.') {
                e.clone()
            } else {
                format!(".{}", e)
            }
        })
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
            "No files with extensions [{}] found in {}.",
            exts_str, dir_path
        ));
    }

    debug!(
        "Directory indexing starting: {} files in '{}' (force={})...",
        files.len(),
        dir_path,
        args.force
    );
    {
        let db_guard = db.lock().await;
        db_guard.insert_collection(&args.collection)?;
    }

    // Use the *absolute path* (lowercased) as doc_id — same scheme as ingest_file.
    // This avoids collisions between README.md vs readme.md and between
    // different directories sharing a filename.
    //
    // `fp` is only needed while building this vector; the pushed tuple does
    // not carry it, so we avoid a per-file PathBuf clone that would be dead.
    let mut prepared = Vec::new();
    for fp in &files {
        let display_name = std::path::Path::new(fp)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let abs =
            std::path::absolute(fp).unwrap_or_else(|_| std::path::Path::new(fp).to_path_buf());
        let doc_id = abs.to_string_lossy().to_lowercase();

        match extract_text_from_file(fp, Some(&args.encoding)) {
            Ok(text) => match chunk_text_exact(
                &text,
                cfg.chunk_size,
                cfg.chunk_overlap,
                &cfg.tokenizer_path,
            ) {
                Ok(chunks) => {
                    debug!(
                        "  Chunked: '{}' (doc_id='{}') → {} chunks",
                        display_name,
                        doc_id,
                        chunks.len()
                    );
                    prepared.push((display_name, doc_id, chunks, None));
                }
                Err(e) => {
                    debug!("  Error chunking '{}': {}", display_name, e);
                    prepared.push((display_name, doc_id, Vec::new(), Some(e.to_string())));
                }
            },
            Err(e) => {
                debug!("  Error reading '{}': {}", display_name, e);
                prepared.push((display_name, doc_id, Vec::new(), Some(e.to_string())));
            }
        }
    }

    let sem = Arc::new(Semaphore::new(cfg.max_concurrent_files));
    let mut tasks: Vec<tokio::task::JoinHandle<anyhow::Result<(String, usize, String)>>> =
        Vec::new();

    for (display_name, doc_id, chunks, err) in prepared {
        let db = Arc::clone(db);
        let cfg = cfg.clone();
        let client = client.clone();
        let sem = Arc::clone(&sem);
        let collection = args.collection.clone();
        let force = args.force;
        let permit = sem.clone().acquire_owned().await.unwrap();
        tasks.push(tokio::spawn(async move {
            let _permit = permit;
            if let Some(e) = err {
                return Ok((display_name, 0, format!("error – {}", e)));
            }
            if chunks.is_empty() {
                return Ok((display_name, 0, "empty file".to_string()));
            }
            {
                let db_guard = db.lock().await;
                if db_guard.doc_exists(&collection, &doc_id)? && !force {
                    return Ok((display_name, 0, "already indexed".to_string()));
                }
            }

            let embeddings = get_embeddings(&client, &cfg, &chunks, &doc_id).await?;
            {
                let mut db_guard = db.lock().await;
                db_guard.insert_chunks(&collection, &doc_id, &chunks, &embeddings)?;
            }

            let action = if force { "re-indexed" } else { "indexed" };
            Ok((
                display_name,
                chunks.len(),
                format!("{} segment {}", chunks.len(), action),
            ))
        }));
    }

    let results = join_all(tasks).await;
    let mut result_lines = Vec::new();
    let mut total_segments = 0;
    for res in results {
        match res {
            Ok(Ok((filename, seg, msg))) => {
                total_segments += seg;
                let prefix = if seg > 0 {
                    "✓"
                } else if msg.contains("already indexed") || msg.contains("skipped") {
                    "⚠"
                } else {
                    "✗"
                };
                result_lines.push(format!("  {} {}: {}", prefix, filename, msg));
            }
            Ok(Err(e)) => {
                result_lines.push(format!("  ✗ file: {}", e));
            }
            Err(e) => {
                result_lines.push(format!("  ✗ task panic: {}", e));
            }
        }
    }

    let summary = format!(
        "Directory indexing complete. {} files · {} chunks · collection: '{}'",
        files.len(),
        total_segments,
        args.collection
    );
    debug!("{}", summary);
    Ok(summary + "\n" + &result_lines.join("\n"))
}
