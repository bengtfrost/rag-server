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

    // Prepare data for each document: (doc_id, chunks, embeddings)
    // We'll skip documents that already exist and force=false.
    let mut to_insert = Vec::new();
    let mut skipped = Vec::new();
    let t0 = Instant::now();

    for (i, (doc_id, doc_text)) in args.ids.iter().zip(args.documents.iter()).enumerate() {
        debug!("Dokument {}/{}: '{}'", i+1, args.ids.len(), doc_id);
        let db_guard = db.lock().await;
        let exists = db_guard.doc_exists(&args.collection, doc_id)?;
        drop(db_guard);

        if exists && !args.force {
            skipped.push(doc_id.clone());
            continue;
        }

        let chunks = chunk_text_exact(doc_text, cfg.chunk_size, cfg.chunk_overlap)?;
        if chunks.is_empty() {
            continue;
        }
        to_insert.push((doc_id.clone(), chunks));
    }

    if to_insert.is_empty() {
        let mut msg = format!("Inga nya dokument att indexera i '{}'.", args.collection);
        if !skipped.is_empty() {
            msg.push_str(&format!("\nHoppades över (redan indexerade): {}", skipped.join(", ")));
        }
        return Ok(msg);
    }

    // Fetch embeddings for all chunks (grouped by document)
    // We'll collect all chunk texts with their doc index
    let mut all_chunks = Vec::new();
    let mut doc_indices = Vec::new(); // parallel to all_chunks, stores which doc it belongs to
    for (doc_idx, (_doc_id, chunks)) in to_insert.iter().enumerate() {
        for chunk in chunks {
            all_chunks.push(chunk.clone());
            doc_indices.push(doc_idx);
        }
    }

    let embeddings = get_embeddings(client, cfg, &all_chunks, "batch").await?;
    // Group embeddings back by document
    let mut doc_embeddings: Vec<Vec<Vec<f32>>> = Vec::new();
    for _ in 0..to_insert.len() {
        doc_embeddings.push(Vec::new());
    }
    for (emb, doc_idx) in embeddings.into_iter().zip(doc_indices) {
        doc_embeddings[doc_idx].push(emb);
    }

    // Prepare final data: (doc_id, chunks, embeddings)
    let mut insert_data = Vec::new();
    for ((doc_id, chunks), emb_list) in to_insert.into_iter().zip(doc_embeddings) {
        insert_data.push((doc_id, chunks, emb_list));
    }

    // Perform batch insert in a single transaction
    let mut db_guard = db.lock().await;
    db_guard.replace_chunks_batch(&args.collection, &insert_data)?;
    drop(db_guard);

    let elapsed = t0.elapsed();
    let total_chunks: usize = insert_data.iter().map(|(_, chunks, _)| chunks.len()).sum();

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