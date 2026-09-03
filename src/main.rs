mod cache;
mod chunker;
mod config;
mod db;
mod embedder;
mod expander;
mod extractor;
mod pipeline;
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

// CLI-struktur
#[derive(Parser)]
#[command(name = "rag-server")]
#[command(about = "Sovereign Rust RAG Server med lokal embedding och reranking")]
#[command(version = "2.3.4")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Skapa en ny samling")]
    CreateCollection(CreateCollectionArgs),

    #[command(about = "Indexera en enskild fil")]
    IngestFile(IngestFileArgs),

    #[command(about = "Indexera alla filer i en katalog")]
    IngestDirectory(IngestDirectoryArgs),

    #[command(about = "Sök i en samling")]
    Query(QueryArgs),

    #[command(about = "Lista alla samlingar")]
    ListCollections,

    #[command(about = "Ta bort dokument från en samling")]
    DeleteDocuments(DeleteDocumentsArgs),

    #[command(about = "Ta bort en hel samling")]
    DeleteCollection(DeleteCollectionArgs),

    #[command(about = "Rensa cache")]
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
    let cfg = Config::from_env()?;
    let db = Arc::new(Mutex::new(Db::new(&cfg.db_path)?));

    // Improved client with connection pooling
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(Duration::from_secs(30))
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
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "sovereign-rag-rust",
                            "version": "2.3.0"
                        }
                    }
                });
                println!("{}", resp);
            }
            "notifications/initialized" => {
                eprintln!("[*] Goose ansluten till Rust RAG.");
            }
            "tools/list" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": list_tools()
                    }
                });
                println!("{}", resp);
            }
            "tools/call" => {
                let tool_name = req["params"]["name"].as_str().unwrap_or("");
                let args = &req["params"]["arguments"];
                eprintln!("[*] Goose anropar verktyg: {}", tool_name);

                match call_tool(tool_name, args.clone(), &cfg, &db, &client).await {
                    Ok(result_text) => {
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": result_text}]
                            }
                        });
                        println!("{}", resp);
                    }
                    Err(e) => {
                        let resp = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32000,
                                "message": e.to_string()
                            }
                        });
                        println!("{}", resp);
                    }
                }
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
            println!("✓ Cache rensad.");
        }
    }
    Ok(())
}
