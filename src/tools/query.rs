use clap::Args;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::Config;
use crate::db::Db;
use crate::embedder::get_embeddings;
use crate::expander::expand_query;
use crate::reranker::rerank;

#[derive(Debug, Deserialize, Args)]
pub struct QueryArgs {
    #[arg(short, long)]
    pub collection: String,
    #[arg(short, long)]
    pub query: String,
    #[arg(short, long, default_value = "5")]
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    // 👇 Ny parameter – valfri URL för reranker
    #[arg(long)]
    pub rerank_url: Option<String>,
}

fn default_top_k() -> usize {
    5
}

pub async fn query(
    db: &Arc<Mutex<Db>>,
    cfg: &Config,
    client: &reqwest::Client,
    args: QueryArgs,
) -> anyhow::Result<String> {
    let optimized_query = expand_query(client, &args.query).await;
    let query_preview = if optimized_query.len() > 80 {
        format!("{}...", &optimized_query[..80])
    } else {
        optimized_query.clone()
    };
    debug!("Query: '{}' i samling '{}'", query_preview, args.collection);

    let t0 = Instant::now();
    let query_emb = get_embeddings(client, cfg, &[optimized_query.clone()], "query").await?;
    let query_emb = query_emb
        .first()
        .ok_or_else(|| anyhow::anyhow!("No embedding returned"))?;

    debug!("Hämtar {} ANN-kandidater...", cfg.rerank_candidates);
    let db_guard = db.lock().await;
    let chunk_ids: Vec<String> = db_guard
        .search(&args.collection, query_emb.clone(), cfg.rerank_candidates)?
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();
    drop(db_guard);

    if chunk_ids.is_empty() {
        return Ok("Hittade inget relevant.".to_string());
    }

    debug!("{} kandidater hämtade, hämtar text...", chunk_ids.len());
    let db_guard = db.lock().await;
    let doc_map = db_guard.get_chunk_texts(&chunk_ids)?;
    drop(db_guard);

    let doc_texts: Vec<String> = doc_map.iter().map(|(_, text, _)| text.clone()).collect();

    // Använd angiven rerank_url eller fallback till cfg.rerank_url
    let rerank_url = args.rerank_url.as_deref().unwrap_or(&cfg.rerank_url);

    let reranked = rerank(
        client,
        rerank_url,
        cfg,
        &optimized_query,
        &doc_texts,
        args.top_k,
    )
    .await?;

    if reranked.is_empty() {
        return Ok("Inga tillräckligt relevanta träffar.".to_string());
    }

    let elapsed = t0.elapsed();
    debug!(
        "Query klar på {}, returnerar {} träffar.",
        format_duration(elapsed),
        reranked.len()
    );

    let mut results = Vec::new();
    for (i, (idx, score)) in reranked.iter().enumerate() {
        if let Some((_id, text, parent)) = doc_map.get(*idx) {
            results.push(format!(
                "[{}] (Källa: {}) Score: {:.4}\n{}",
                i + 1,
                parent,
                score,
                text
            ));
        }
    }

    Ok(results.join("\n\n---\n\n"))
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}
