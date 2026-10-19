use clap::Args;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::debug;

use crate::chunker::chunk_text_exact;
use crate::config::Config;
use crate::db::Db;
use crate::embedder::get_embeddings;

#[derive(Debug, Deserialize, Args)]
pub struct AddDocumentsArgs {
    #[arg(short, long)]
    pub collection: String,
    #[arg(short, long)]
    pub ids: Vec<String>,
    #[arg(short, long)]
    pub documents: Vec<String>,
    #[arg(short, long)]
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
        return Err(anyhow::anyhow!("Number of IDs and documents do not match."));
    }

    {
        let db_guard = db.lock().await;
        db_guard.insert_collection(&args.collection)?;
    }

    // One query for every id — instead of a lock+query per document.
    let existing = {
        let db_guard = db.lock().await;
        db_guard.existing_doc_ids(&args.collection, &args.ids)?
    };

    let mut to_insert: Vec<(String, Vec<String>)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let t0 = Instant::now();

    for (i, (doc_id, doc_text)) in args.ids.iter().zip(args.documents.iter()).enumerate() {
        debug!("Document {}/{}: '{}'", i + 1, args.ids.len(), doc_id);
        if existing.contains(doc_id) && !args.force {
            skipped.push(doc_id.clone());
            continue;
        }
        let chunks = chunk_text_exact(
            doc_text,
            cfg.chunk_size,
            cfg.chunk_overlap,
            &cfg.tokenizer_path,
        )?;
        if chunks.is_empty() {
            continue;
        }
        to_insert.push((doc_id.clone(), chunks));
    }

    if to_insert.is_empty() {
        let mut msg = format!("No new documents to index in '{}'.", args.collection);
        if !skipped.is_empty() {
            msg.push_str(&format!(
                "\nSkipped (already indexed): {}",
                skipped.join(", ")
            ));
        }
        return Ok(msg);
    }

    // Flatten chunks for embedding; remember which doc each came from.
    let mut all_chunks: Vec<String> = Vec::new();
    let mut doc_indices: Vec<usize> = Vec::new();
    for (doc_idx, (_doc_id, chunks)) in to_insert.iter().enumerate() {
        for chunk in chunks {
            all_chunks.push(chunk.clone());
            doc_indices.push(doc_idx);
        }
    }

    let embeddings = get_embeddings(client, cfg, &all_chunks, "batch").await?;

    // Regroup embeddings back per document.
    let mut doc_embeddings: Vec<Vec<Vec<f32>>> = vec![Vec::new(); to_insert.len()];
    for (emb, doc_idx) in embeddings.into_iter().zip(doc_indices) {
        doc_embeddings[doc_idx].push(emb);
    }

    let insert_data: Vec<(String, Vec<String>, Vec<Vec<f32>>)> = to_insert
        .into_iter()
        .zip(doc_embeddings)
        .map(|((doc_id, chunks), embs)| (doc_id, chunks, embs))
        .collect();

    {
        let mut db_guard = db.lock().await;
        db_guard.replace_chunks_batch(&args.collection, &insert_data)?;
    }

    let elapsed = t0.elapsed();
    let total_chunks: usize = insert_data.iter().map(|(_, c, _)| c.len()).sum();

    let mut msg = format!(
        "✓ Indexed {} chunks in '{}' in {}.",
        total_chunks,
        args.collection,
        format_duration(elapsed)
    );
    if !skipped.is_empty() {
        msg.push_str(&format!(
            "\nWarning: Already indexed (skipped): {}",
            skipped.join(", ")
        ));
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
