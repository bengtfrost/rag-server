use reqwest::Client;
use serde_json::json;
use std::time::Instant;
use tracing::debug;
use crate::config::Config;

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

    let mut all_embeddings = Vec::with_capacity(total);
    let batch_size = cfg.embed_batch_size;
    let total_batches = (total + batch_size - 1) / batch_size;
    let start = Instant::now();

    for (batch_num, chunk) in texts.chunks(batch_size).enumerate() {
        let batch_texts = chunk.to_vec();
        let payload = json!({
            "input": batch_texts,
            "model": cfg.embed_model,
        });

        let resp = client.post(&cfg.embed_url)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(cfg.timeout_secs))
            .send()
            .await?;
        let data: serde_json::Value = resp.json().await?;
        let data_array = data["data"].as_array()
            .ok_or_else(|| anyhow::anyhow!("Missing 'data' in embedding response"))?;

        let mut sorted = data_array.iter().collect::<Vec<_>>();
        sorted.sort_by_key(|v| v["index"].as_u64().unwrap_or(0));

        for item in sorted {
            let emb: Vec<f32> = item["embedding"].as_array()
                .ok_or_else(|| anyhow::anyhow!("Missing embedding"))?
                .iter()
                .map(|v| v.as_f64().unwrap_or(0.0) as f32)
                .collect();
            all_embeddings.push(emb);
        }

        let elapsed = start.elapsed().as_secs_f64();
        let done = std::cmp::min((batch_num + 1) * batch_size, total);
        let pct = done as f64 / total as f64;
        let bar = progress_bar(pct, 30);
        let eta = if batch_num == 0 {
            "ETA: calculating...".to_string()
        } else {
            let elapsed_per_item = elapsed / done as f64;
            let remaining = (total - done) as f64 * elapsed_per_item;
            format!("ETA: {}s", remaining as u64)
        };
        debug!("{} Embeddings batch {}/{} {} {}", label, batch_num + 1, total_batches, bar, eta);
    }

    Ok(all_embeddings)
}

fn progress_bar(pct: f64, width: usize) -> String {
    let filled = (pct * width as f64) as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(width - filled);
    format!("[{}] {:.0}%", bar, pct * 100.0)
}