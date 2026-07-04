use clap::Args;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::debug;

use crate::cache::QUERY_CACHE;
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
    #[arg(long)]
    pub rerank_url: Option<String>,
    #[arg(long)]
    pub hybrid: bool,
    #[arg(long, default_value = "0.7")]
    pub vector_weight: f64,
    #[arg(long, default_value = "0.3")]
    pub bm25_weight: f64,
    // NEW: skip cache
    #[arg(long)]
    pub no_cache: bool,
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

    // Check cache for hybrid search results (unless no_cache)
    let cache_key = format!(
        "{}:{}:{}:{}:{}:{}",
        args.collection,
        optimized_query,
        args.hybrid,
        args.vector_weight,
        args.bm25_weight,
        cfg.rerank_candidates
    );

    let chunk_ids: Vec<String> = if !args.no_cache {
        let mut cache = QUERY_CACHE.lock().await;
        if let Some(cached) = cache.get(&cache_key) {
            debug!("Cache hit for query");
            cached
        } else {
            // Run search and cache
            let results = run_search(db, cfg, &args, query_emb, &optimized_query).await?;
            cache.insert(cache_key, results.clone());
            results
        }
    } else {
        run_search(db, cfg, &args, query_emb, &optimized_query).await?
    };

    if chunk_ids.is_empty() {
        return Ok("Hittade inget relevant.".to_string());
    }

    debug!("{} kandidater hämtade, hämtar text...", chunk_ids.len());
    let db_guard = db.lock().await;
    let doc_map = db_guard.get_chunk_texts(&chunk_ids)?;
    drop(db_guard);

    let doc_texts: Vec<String> = doc_map.iter().map(|(_, text, _)| text.clone()).collect();

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

async fn run_search(
    db: &Arc<Mutex<Db>>,
    cfg: &Config,
    args: &QueryArgs,
    query_emb: &Vec<f32>,
    optimized_query: &str,
) -> anyhow::Result<Vec<String>> {
    let db_guard = db.lock().await;

    let chunk_ids: Vec<String> = if args.hybrid {
        debug!(
            "Använder hybrid search (BM25 + vector) med vikter {} / {}",
            args.vector_weight, args.bm25_weight
        );
        // Use async parallel hybrid search
        let results = db_guard
            .hybrid_search_async(
                &args.collection,
                query_emb.clone(),
                optimized_query,
                cfg.rerank_candidates * 2,
                args.vector_weight,
                args.bm25_weight,
            )
            .await?;
        results.iter().map(|(id, _, _)| id.clone()).collect()
    } else {
        db_guard
            .search(&args.collection, query_emb.clone(), cfg.rerank_candidates)?
            .into_iter()
            .map(|(id, _, _)| id)
            .collect()
    };

    drop(db_guard);
    Ok(chunk_ids)
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else {
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

