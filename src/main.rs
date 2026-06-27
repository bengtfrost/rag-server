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

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .build()?;

    eprintln!("[*] Sovereign Rust RAG Server startad...");

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let req_text = line?;
        let req: Value = match serde_json::from_str(&req_text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = req["method"].as_str().unwrap_or("");
        let id = &req["id"];

        match method {
            "initialize" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {}
                        },
                        "serverInfo": {
                            "name": "sovereign-rag-rust",
                            "version": "2.1.2"
                        }
                    }
                });
                println!("{}", resp);
            }

            "notifications/initialized" => {
                // Goose bekräftar att anslutningen är klar
                eprintln!("[*] Goose ansluten till Rust RAG.");
            }

            "tools/list" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [
                            {
                                "name": "create_collection",
                                "description": "Skapa en ny juridisk eller kod-samling",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "name": { "type": "string", "description": "Namn på samlingen" }
                                    },
                                    "required": ["name"]
                                }
                            },
                            {
                                "name": "ingest_file",
                                "description": "Läs in, dela upp och indexera en fil lokalt via iGPU",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "collection": { "type": "string" },
                                        "path": { "type": "string", "description": "Absolut sökväg till filen" }
                                    },
                                    "required": ["collection", "path"]
                                }
                            },
                            {
                                "name": "list_collections",
                                "description": "Visa alla tillgängliga kunskapsbaser",
                                "inputSchema": { "type": "object", "properties": {} }
                            },
                            {
                                "name": "query",
                                "description": "Sök i samlingen med semantisk precision och reranking",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "collection": { "type": "string" },
                                        "query": { "type": "string" }
                                    },
                                    "required": ["collection", "query"]
                                }
                            }
                        ]
                    }
                });
                println!("{}", resp);
            }

            "tools/call" => {
                let tool_name = req["params"]["name"].as_str().unwrap_or("");
                let args = &req["params"]["arguments"];
                eprintln!("[*] Goose anropar verktyg: {}", tool_name);

                let result = match tool_name {
                    "create_collection" => {
                        let name = args["name"].as_str().unwrap_or("");
                        db.add_collection(name)?;
                        json!([{"type": "text", "text": format!("Samlingen '{}' är skapad och klar.", name)}])
                    }
                    "list_collections" => {
                        let colls = db.list_collections()?;
                        let text = colls
                            .iter()
                            .map(|(n, c)| format!("• {}: {} dokument", n, c))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let final_text = if text.is_empty() {
                            "Inga samlingar hittades.".to_string()
                        } else {
                            text
                        };
                        json!([{"type": "text", "text": final_text}])
                    }
                    "ingest_file" => {
                        let coll = args["collection"].as_str().unwrap_or("");
                        let path = args["path"].as_str().unwrap_or("");
                        let content = std::fs::read_to_string(path)?;
                        let chunks =
                            chunker.chunk_text(&content, cfg.chunk_size, cfg.chunk_overlap);

                        let mut all_embs = Vec::new();
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
                        json!([{"type": "text", "text": format!("Filen '{}' har indexerats i '{}'.", path, coll)}])
                    }
                    "query" => {
                        let coll = args["collection"].as_str().unwrap_or("");
                        let q_text = args["query"].as_str().unwrap_or("");

                        let emb_resp = client
                            .post(&cfg.embed_url)
                            .json(&json!({"input": [q_text], "model": "bge-m3"}))
                            .send()
                            .await?
                            .json::<Value>()
                            .await?;
                        let q_emb: Vec<f32> =
                            serde_json::from_value(emb_resp["data"][0]["embedding"].clone())?;

                        let hits = db.search(coll, q_emb, cfg.rerank_candidates)?;
                        let docs: Vec<String> = hits.iter().map(|h| h.1.clone()).collect();

                        if docs.is_empty() {
                            json!([{"type": "text", "text": "Hittade ingen relevant kontext i databasen."}])
                        } else {
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
                                    "[{}] (Relevans: {:.4})\n{}\n\n",
                                    i + 1,
                                    score,
                                    docs[idx]
                                ));
                            }
                            json!([{"type": "text", "text": final_text}])
                        }
                    }
                    _ => json!([{"type": "text", "text": "Verktyget hittades inte."}]),
                };

                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "content": result }
                });
                println!("{}", resp);
            }
            _ => {}
        }
    }
    Ok(())
}
