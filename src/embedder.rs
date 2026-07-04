use crate::config::Config;
use futures::stream::{self, StreamExt};
use reqwest::Client;
use serde_json::json;
use std::time::Instant;
use tracing::debug;

pub async fn get_embeddings(
    client: &Client,
    cfg: &Config,
    texts: &[String],
    label: &str,
) -> anyhow::Result<Vec<Vec<f32>>> {
    let total = texts.len();
    if total == 0 {
        return Ok(Vec::new());
    }

    let batch_size = cfg.embed_batch_size;
    let max_concurrent = cfg.max_concurrent_requests.unwrap_or(4);
    let mut all_embeddings = Vec::with_capacity(total);
    let start = Instant::now();

    let batches: Vec<Vec<String>> = texts
        .chunks(batch_size)
        .map(|chunk| chunk.to_vec())
        .collect();

    let total_batches = batches.len();
    let client_clone = client.clone();
    let cfg_clone = cfg.clone();

    let batch_futures = batches.into_iter().enumerate().map(|(batch_num, batch)| {
        let client = client_clone.clone();
        let cfg = cfg_clone.clone();
        let label = label.to_string();

        async move {
            let payload = json!({
                "input": batch,
                "model": cfg.embed_model,
            });

            let resp = client
                .post(&cfg.embed_url)
                .json(&payload)
                .timeout(std::time::Duration::from_secs(cfg.timeout_secs))
                .send()
                .await?;

            let data: serde_json::Value = resp.json().await?;
            let data_array = data["data"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Missing 'data' in embedding response"))?;

            let mut sorted = data_array.iter().collect::<Vec<_>>();
            sorted.sort_by_key(|v| v["index"].as_u64().unwrap_or(0));

            let mut batch_embeddings = Vec::with_capacity(sorted.len());
            for item in sorted {
                let emb: Vec<f32> = item["embedding"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("Missing embedding"))?
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                    .collect();
                batch_embeddings.push(emb);
            }

            let _elapsed = start.elapsed().as_secs_f64(); // prefix with underscore
            let pct = (batch_num + 1) as f64 / total_batches as f64;
            let bar = progress_bar(pct, 30);
            debug!(
                "{} Embeddings batch {}/{} {}",
                label,
                batch_num + 1,
                total_batches,
                bar
            );

            Ok::<_, anyhow::Error>(batch_embeddings)
        }
    });

    let batch_results: Vec<Vec<Vec<f32>>> = stream::iter(batch_futures)
        .buffer_unordered(max_concurrent)
        .collect::<Vec<Result<Vec<Vec<f32>>, anyhow::Error>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    for batch in batch_results {
        all_embeddings.extend(batch);
    }

    Ok(all_embeddings)
}

fn progress_bar(pct: f64, width: usize) -> String {
    let filled = (pct * width as f64) as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(width - filled);
    format!("[{}] {:.0}%", bar, pct * 100.0)
}

