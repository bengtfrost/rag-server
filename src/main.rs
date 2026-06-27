mod config;
mod db;
mod chunker;
mod embedder;
mod reranker;
mod expander;
mod extractor;
mod tools;

use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, error};
use serde_json::{json, Value};
use anyhow::Result;
use tokio::io::AsyncBufReadExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("rag_server=debug,info")
        .with_writer(std::io::stderr)
        .init();

    info!("Starting RAG MCP server v2.0.0");

    let cfg = config::Config::from_env()?;
    debug!("Loaded config: {:?}", cfg);

    let mut db = db::Db::new(&cfg.db_path)?;
    db.init(&cfg.sqlite_vec_path)?;
    let db_arc = Arc::new(Mutex::new(db));

    let http_client = reqwest::Client::new();

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        debug!("Received: {}", line);

        let response = match handle_request(&line, &cfg, &db_arc, &http_client).await {
            Ok(resp) => resp,
            Err(e) => {
                error!("Request handling failed: {}", e);
                if let Ok(parsed) = serde_json::from_str::<Value>(&line) {
                    if let Some(id) = parsed.get("id").cloned() {
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32603,
                                "message": format!("Internal error: {}", e)
                            }
                        })
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        };

        let out = serde_json::to_string(&response)?;
        println!("{}", out);
        std::io::stdout().flush()?;
    }

    Ok(())
}

async fn handle_request(
    line: &str,
    cfg: &config::Config,
    db: &Arc<Mutex<db::Db>>,
    client: &reqwest::Client,
) -> Result<Value> {
    let parsed: Value = serde_json::from_str(line)?;
    let id = parsed["id"].clone();
    let method = parsed["method"].as_str().ok_or_else(|| anyhow::anyhow!("Missing method"))?;

    match method {
        "initialize" => {
            Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "rag-bge", "version": "2.0.0" }
                }
            }))
        }
        "notifications/initialized" => {
            Ok(json!({ "jsonrpc": "2.0", "id": id }))
        }
        "tools/list" => {
            let tools = tools::list_tools();
            Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "tools": tools }
            }))
        }
        "tools/call" => {
            let params = &parsed["params"];
            let tool_name = params["name"].as_str().ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;
            let args = params["arguments"].clone();

            let result_text = tools::call_tool(
                tool_name,
                args,
                cfg,
                db,
                client,
            ).await?;

            Ok(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": result_text }],
                    "isError": false
                }
            }))
        }
        _ => {
            Err(anyhow::anyhow!("Unsupported method: {}", method))
        }
    }
}