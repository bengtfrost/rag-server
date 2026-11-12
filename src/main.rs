mod cache;
mod chunker;
mod config;
mod db;
mod embedder;
mod expander;
mod extractor;
mod reranker;
mod tools;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::io::{self, BufRead};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use tracing_subscriber::{EnvFilter, fmt};

use crate::config::Config;
use crate::db::Db;
use crate::tools::*;

#[derive(Parser)]
#[command(name = "rag-server")]
#[command(about = "Sovereign Rust RAG Server with local embedding and reranking")]
#[command(version = "2.4.1")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Create a new collection")]
    CreateCollection(CreateCollectionArgs),
    #[command(about = "Index a single file")]
    IngestFile(IngestFileArgs),
    #[command(about = "Index all files in a directory")]
    IngestDirectory(IngestDirectoryArgs),
    #[command(about = "Search a collection")]
    Query(QueryArgs),
    #[command(about = "List all collections")]
    ListCollections,
    #[command(about = "Remove documents from a collection")]
    DeleteDocuments(DeleteDocumentsArgs),
    #[command(about = "Delete an entire collection")]
    DeleteCollection(DeleteCollectionArgs),
    #[command(about = "Clear cache")]
    ClearCache,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs MUST go to stderr — stdout is the JSON-RPC transport.
    // `println!` on stdout is line-buffered, so every JSON-RPC response is
    // flushed automatically; any stray log line on stdout would corrupt
    // the protocol.
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("rag_server_mcp=debug,info")),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();

    // `JoinError`'s Display impl shows the panic payload but not the
    // source location — record it here so a panic in a tool has an
    // actionable file:line:col in stderr.
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        eprintln!("[PANIC] {location}: {payload}");
    }));

    let cli = Cli::parse();
    if let Some(command) = cli.command {
        run_cli(command).await?;
    } else {
        run_server().await?;
    }
    Ok(())
}

async fn run_server() -> anyhow::Result<()> {
    let cfg = Arc::new(Config::from_env()?);
    let db = Arc::new(Mutex::new(Db::new(&cfg.db_path)?));

    let client = Arc::new(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(cfg.timeout_secs))
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?,
    );

    info!(
        db = %cfg.db_path,
        tokenizer = %cfg.tokenizer_path,
        embed_url = %cfg.embed_url,
        rerank_url = %cfg.rerank_url,
        expand_url = %cfg.expand_url,
        timeout_secs = cfg.timeout_secs,
        "Sovereign Rust RAG Server listening on stdio"
    );

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let req_text = line?;
        let req: Value = match serde_json::from_str(&req_text) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "Dropping malformed JSON-RPC line");
                continue;
            }
        };

        let method = req["method"].as_str().unwrap_or("");
        debug!(method, "Received JSON-RPC request");

        match method {
            "initialize" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": &req["id"],
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "sovereign-rag-rust",
                            "version": "2.4.1"
                        }
                    }
                });
                println!("{}", resp);
            }
            "notifications/initialized" => {
                info!("Client completed MCP initialization handshake");
            }
            "tools/list" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": &req["id"],
                    "result": { "tools": list_tools() }
                });
                println!("{}", resp);
            }
            "tools/call" => {
                let tool_name = req["params"]["name"].as_str().unwrap_or("").to_string();
                let args = req["params"]["arguments"].clone();
                let id = req["id"].clone();

                let cfg = Arc::clone(&cfg);
                let db = Arc::clone(&db);
                let client = Arc::clone(&client);

                info!(tool = %tool_name, "Executing tool call");

                // Isolate tool execution in a spawned task so a panic
                // becomes a JSON-RPC error for this one request instead of
                // terminating the server. Requires `panic = "unwind"` in
                // the active profile (the default), which [profile.release]
                // in Cargo.toml leaves untouched.
                let handle =
                    tokio::spawn(
                        async move { call_tool(&tool_name, args, &cfg, &db, &client).await },
                    );

                let resp = match handle.await {
                    Ok(Ok(result_text)) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": result_text}]
                        }
                    }),
                    Ok(Err(e)) => {
                        warn!(error = %e, "Tool call returned an error");
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32000,
                                "message": e.to_string()
                            }
                        })
                    }
                    Err(join_err) => {
                        warn!(error = %join_err, "Tool call task failed (panic or cancellation)");
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32603,
                                "message": format!("Internal error: {}", join_err)
                            }
                        })
                    }
                };
                println!("{}", resp);
            }
            other => {
                debug!(method = other, "Ignoring unhandled method");
            }
        }
    }
    info!("stdin closed; shutting down cleanly");
    Ok(())
}

async fn run_cli(command: Commands) -> anyhow::Result<()> {
    let cfg = Config::from_env()?;
    let db = Arc::new(Mutex::new(Db::new(&cfg.db_path)?));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(Duration::from_secs(30))
        .build()?;

    match command {
        Commands::CreateCollection(args) => {
            let result = create_collection::create_collection(&db, args).await?;
            println!("{}", result);
        }
        Commands::IngestFile(args) => {
            let result = ingest_file::ingest_file(&db, &cfg, &client, args).await?;
            println!("{}", result);
        }
        Commands::IngestDirectory(args) => {
            let result = ingest_directory::ingest_directory(&db, &cfg, &client, args).await?;
            println!("{}", result);
        }
        Commands::Query(args) => {
            let result = query::query(&db, &cfg, &client, args).await?;
            println!("{}", result);
        }
        Commands::ListCollections => {
            let result = list_collections::list_collections(&db).await?;
            println!("{}", result);
        }
        Commands::DeleteDocuments(args) => {
            let result = delete_documents::delete_documents(&db, args).await?;
            println!("{}", result);
        }
        Commands::DeleteCollection(args) => {
            let result = delete_collection::delete_collection(&db, args).await?;
            println!("{}", result);
        }
        Commands::ClearCache => {
            cache::QUERY_CACHE.clear();
            println!("✓ Cache cleared.");
        }
    }
    Ok(())
}
