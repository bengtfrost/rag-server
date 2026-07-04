use reqwest::Client;
use serde_json::json;
use tracing::debug;
use crate::config::Config;

pub async fn rerank(
    client: &Client,
    rerank_url: &str,
    cfg: &Config,
    query: &str,
    documents: &[String],
    top_n: usize,
) -> anyhow::Result<Vec<(usize, f64)>> {
    if documents.is_empty() {
        return Ok(Vec::new());
    }

    // Adaptive reranking: skip if few candidates
    let min_candidates = cfg.rerank_min_candidates.unwrap_or(3);
    if documents.len() <= min_candidates {
        debug!("Skipping reranking: only {} candidates", documents.len());
        return Ok(documents
            .iter()
            .enumerate()
            .map(|(i, _)| (i, 1.0))
            .collect());
    }

    debug!("Reranking {} candidates via Agentgateway...", documents.len());

    let payload = json!({
        "model": cfg.rerank_model,
        "query": query,
        "documents": documents,
    });

    let resp = client
        .post(rerank_url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(cfg.timeout_secs))
        .send()
        .await?;

    let data: serde_json::Value = resp.json().await?;
    let results = data["results"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing 'results' in rerank response"))?;

    let mut indexed = Vec::new();
    for item in results {
        let idx = item["index"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Missing index"))? as usize;
        let score = item["relevance_score"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("Missing relevance_score"))?;
        indexed.push((idx, score));
    }

    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let filtered: Vec<(usize, f64)> = indexed
        .into_iter()
        .filter(|(_, score)| *score > cfg.rerank_min_score)
        .take(top_n)
        .collect();

    debug!(
        "Reranking done: {} hits above threshold {}",
        filtered.len(),
        cfg.rerank_min_score
    );
    Ok(filtered)
}