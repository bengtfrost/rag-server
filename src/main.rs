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

use crate::config::Config;
use crate::db::Db;
use crate::tools::*;

#[derive(Parser)]
#[command(name = "rag-server")]
#[command(about = "Sovereign Rust RAG Server with local embedding and reranking")]
#[command(version = "2.4.0")]
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

    eprintln!("[*] Sovereign Rust RAG Server startad...");

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let req_text = line?;
        let req: Value = match serde_json::from_str(&req_text) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let method = req["method"].as_str().unwrap_or("");

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
                            "version": "2.4.0"
                        }
                    }
                });
                println!("{}", resp);
            }
            "notifications/initialized" => {
                eprintln!("[*] Client initialized connection.");
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

                eprintln!("[*] Executing tool call: {}", tool_name);

                // Spawn so a panic in a tool becomes an isolated task failure
                // instead of taking down the whole server process.
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
                    Ok(Err(e)) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": e.to_string()
                        }
                    }),
                    Err(join_err) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32603,
                            "message": format!("Internal error: {}", join_err)
                        }
                    }),
                };
                println!("{}", resp);
            }
            _ => {}
        }
    }
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
