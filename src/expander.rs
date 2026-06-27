use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};
use std::time::Duration;
use regex::Regex;

const EXPAND_URL: &str = "http://localhost:4000/v1/chat/completions";

pub async fn expand_query(client: &Client, query: &str) -> String {
    let broad_keywords = vec![
        "berätta om", "vad är", "förklara", "beskriv", "vad handlar om",
        "vad innebär", "vad betyder", "hur fungerar", "vad gör",
    ];
    let query_lower = query.to_lowercase();
    let needs_expansion = broad_keywords.iter().any(|kw| query_lower.contains(kw));
    if !needs_expansion {
        return query.to_string();
    }

    debug!("Broad query detected. Expanding: '{}'", query);

    let prompt = format!(
        "Du är en sökmotorsassistent. Generera 3-5 synonymer eller relaterade juridiska/tekniska sökord på svenska \
        för att bredda en sökning på: \"{}\". Svara ENDAST med sökorden separerade med mellanslag. \
        Ingen introduktion, inga punktlistor och inga citattecken.",
        query
    );
    let payload = json!({
        "model": "local-llama-server",
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.1,
        "max_tokens": 50,
    });

    match client.post(EXPAND_URL)
        .json(&payload)
        .header("Authorization", "Bearer sk-unused")
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                if let Some(content) = data["choices"][0]["message"]["content"].as_str() {
                    let expanded = content.trim();
                    let expanded = Regex::new(r#"["']"#).unwrap().replace_all(expanded, "").to_string();
                    let expanded = expanded.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !expanded.is_empty() && expanded.to_lowercase() != "null" {
                        let optimized = format!("{} {}", query, expanded);
                        debug!("Query expanded to: '{}'", optimized);
                        return optimized;
                    }
                }
            }
        }
        Err(e) => {
            warn!("Query expansion API call failed ({}). Using static fallback.", e);
        }
    }

    let optimized = if query_lower.contains("rf") || query_lower.contains("regeringsform") || query_lower.contains("lag") {
        format!("{} grundlag författning lagstiftning rättskälla paragrafer riksdag", query)
    } else {
        format!("{} definition förklaring sammanfattning bakgrund information", query)
    };
    debug!("Query expanded (fallback) to: '{}'", optimized);
    optimized
}