use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;
use std::time::Instant;

use crate::config::Config;
use crate::db::Db;
use crate::chunker::chunk_text_exact;
use crate::embedder::get_embeddings;

#[derive(Debug, Deserialize)]
pub struct AddDocumentsArgs {
    pub collection: String,
    pub ids: Vec<String>,
    pub documents: Vec<String>,
    #[serde(default)]
    pub force: bool,
}

pub async fn add_documents(
    db: &Arc<Mutex<Db>>,
    cfg: &Config,
    client: &reqwest::Client,
    args: AddDocumentsArgs,
) -> anyhow::Result<String> {
    if args.ids.len() != args.documents.len() {
        return Err(anyhow::anyhow!("Antal ids och dokument stämmer inte överens."));
    }

    let db_guard = db.lock().await;
    db_guard.insert_collection(&args.collection)?;
    drop(db_guard);

    let mut total_chunks = 0;
    let mut skipped = Vec::new();
    let t0 = Instant::now();

    for (i, (doc_id, doc_text)) in args.ids.iter().zip(args.documents.iter()).enumerate() {
        debug!("Dokument {}/{}: '{}'", i+1, args.ids.len(), doc_id);
        let db_guard = db.lock().await;
        if db_guard.doc_exists(&args.collection, doc_id)? && !args.force {
            skipped.push(doc_id.clone());
            continue;
        }
        drop(db_guard);

        let chunks = chunk_text_exact(doc_text, cfg.chunk_size, cfg.chunk_overlap)?;
        if chunks.is_empty() {
            continue;
        }
        let embeddings = get_embeddings(client, cfg, &chunks, doc_id).await?;
        let db_guard = db.lock().await;
        db_guard.insert_chunks(&args.collection, doc_id, &chunks, &embeddings)?;
        total_chunks += chunks.len();
    }

    let elapsed = t0.elapsed();
    let mut msg = format!("✓ Indexerade {} segment i '{}' på {}.", total_chunks, args.collection, format_duration(elapsed));
    if !skipped.is_empty() {
        msg.push_str(&format!("\nVarning: Redan indexerade (hoppades över): {}", skipped.join(", ")));
    }
    Ok(msg)
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}