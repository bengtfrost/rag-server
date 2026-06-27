mod chunker;
mod config;
mod db;

use crate::chunker::BgeChunker;
use crate::config::Config;
use crate::db::VectorDB;
use serde_json::{Value, json};
use std::io::{self, BufRead};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = Config::from_env()?;
    let chunker = BgeChunker::new(&cfg.tokenizer_path)?;
    let db = VectorDB::new(&cfg.db_path)?;

    // Vi använder timeout från config
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .build()?;

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let req_text = line?;
        let req: Value = serde_json::from_str(&req_text)?;
        let method = req["method"].as_str().unwrap_or("");
        let id = &req["id"];

        match method {
            "initialize" => {
                let resp = json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "sovereign-rag-rust", "version": "2.1.1" }
                    }
                });
                println!("{}", resp);
            }
            "tools/list" => {
                let resp = json!({
                    "jsonrpc": "2.0", "id": id,
                    "result": { "tools": [
                        { "name": "create_collection", "description": "Create a new KB", "inputSchema": { "type": "object", "properties": {"name": {"type":"string"}}, "required": ["name"] }},
                        { "name": "ingest_file", "description": "Chunk and index a file", "inputSchema": { "type": "object", "properties": {"collection": {"type":"string"}, "path": {"type":"string"}}, "required": ["collection", "path"] }},
                        { "name": "list_collections", "description": "List KBs", "inputSchema": { "type": "object", "properties": {} }},
                        { "name": "query", "description": "Search KBs", "inputSchema": { "type": "object", "properties": {"collection": {"type":"string"}, "query": {"type":"string"}}, "required": ["collection", "query"] }}
                    ]}
                });
                println!("{}", resp);
            }
            "tools/call" => {
                let tool_name = req["params"]["name"].as_str().unwrap_or("");
                let args = &req["params"]["arguments"];

                let result = match tool_name {
                    "create_collection" => {
                        let name = args["name"].as_str().unwrap_or("");
                        db.add_collection(name)?;
                        json!([{"type": "text", "text": format!("Collection {} ready", name)}])
                    }
                    "ingest_file" => {
                        let coll = args["collection"].as_str().unwrap_or("");
                        let path = args["path"].as_str().unwrap_or("");
                        let content = std::fs::read_to_string(path)?;
                        let chunks =
                            chunker.chunk_text(&content, cfg.chunk_size, cfg.chunk_overlap);

                        let mut all_embs = Vec::new();

                        // BATCHING: Vi skickar chunks i grupper om 'embed_batch_size' (t.ex. 8)
                        for batch in chunks.chunks(cfg.embed_batch_size) {
                            let emb_resp = client
                                .post(&cfg.embed_url)
                                .json(&json!({"input": batch, "model": "bge-m3"}))
                                .send()
                                .await?
                                .json::<Value>()
                                .await?;

                            let embs: Vec<Vec<f32>> = serde_json::from_value(json!(
                                emb_resp["data"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .map(|d| &d["embedding"])
                                    .collect::<Vec<_>>()
                            ))?;
                            all_embs.extend(embs);
                        }

                        db.insert_chunks(coll, path, chunks, all_embs)?;
                        json!([{"type": "text", "text": format!("Successfully ingested {}", path)}])
                    }
                    "list_collections" => {
                        let colls = db.list_collections()?;
                        let text = colls
                            .iter()
                            .map(|(n, c)| format!("• {}: {} documents", n, c))
                            .collect::<Vec<_>>()
                            .join("\n");
                        json!([{"type": "text", "text": text.if_empty("No collections found.")}])
                    }
                    "query" => {
                        let coll = args["collection"].as_str().unwrap_or("");
                        let q_text = args["query"].as_str().unwrap_or("");

                        // 1. Embed query
                        let emb_resp = client
                            .post(&cfg.embed_url)
                            .json(&json!({"input": [q_text], "model": "bge-m3"}))
                            .send()
                            .await?
                            .json::<Value>()
                            .await?;
                        let q_emb: Vec<f32> =
                            serde_json::from_value(emb_resp["data"][0]["embedding"].clone())?;

                        // 2. Search (använder rerank_candidates från config)
                        let hits = db.search(coll, q_emb, cfg.rerank_candidates)?;
                        let docs: Vec<String> = hits.iter().map(|h| h.1.clone()).collect();

                        if docs.is_empty() {
                            json!([{"type": "text", "text": "No relevant context found in database."}])
                        } else {
                            // 3. Rerank via iGPU
                            let rr_resp = client
                                .post(&cfg.rerank_url)
                                .json(&json!({"query": q_text, "documents": docs}))
                                .send()
                                .await?
                                .json::<Value>()
                                .await?;
                            let rr_results = rr_resp["results"].as_array().unwrap();

                            let mut final_text = String::new();
                            for (i, res) in rr_results.iter().take(5).enumerate() {
                                let idx = res["index"].as_u64().unwrap() as usize;
                                let score = res["relevance_score"].as_f64().unwrap();
                                final_text.push_str(&format!(
                                    "[{}] (Score: {:.4})\n{}\n\n",
                                    i + 1,
                                    score,
                                    docs[idx]
                                ));
                            }
                            json!([{"type": "text", "text": final_text}])
                        }
                    }
                    _ => json!([{"type": "text", "text": "Error: Tool not found"}]),
                };

                let resp = json!({ "jsonrpc": "2.0", "id": id, "result": { "content": result }});
                println!("{}", resp);
            }
            _ => {}
        }
    }
    Ok(())
}

// Helper för snyggare listning
trait StringExt {
    fn if_empty(self, fallback: &str) -> String;
}
impl StringExt for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.to_string()
        } else {
            self
        }
    }
}
