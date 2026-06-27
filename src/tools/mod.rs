use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;
use anyhow::Result;

use crate::config::Config;
use crate::db::Db;

mod create_collection;
mod ingest_file;
mod ingest_directory;
mod add_documents;
mod query;
mod list_collections;
mod delete_documents;
mod delete_collection;

pub fn list_tools() -> Vec<serde_json::Value> {
    vec![
        tool_descriptor(
            "create_collection",
            "Skapa en ny RAG-samling",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        ),
        tool_descriptor(
            "ingest_file",
            "Läs och indexera en fil direkt från disk. Sätt force=true för att tvinga re-indexering.",
            json!({
                "type": "object",
                "properties": {
                    "collection": {"type": "string"},
                    "file_path": {"type": "string"},
                    "document_id": {"type": "string"},
                    "encoding": {"type": "string"},
                    "force": {"type": "boolean"}
                },
                "required": ["collection", "file_path"]
            }),
        ),
        tool_descriptor(
            "ingest_directory",
            "Indexera alla matchande filer i en katalog med parallell bearbetning.",
            json!({
                "type": "object",
                "properties": {
                    "collection": {"type": "string"},
                    "directory_path": {"type": "string"},
                    "file_extensions": {"type": "array", "items": {"type": "string"}},
                    "encoding": {"type": "string"},
                    "force": {"type": "boolean"}
                },
                "required": ["collection", "directory_path"]
            }),
        ),
        tool_descriptor(
            "add_documents",
            "Indexera råtext-strängar direkt.",
            json!({
                "type": "object",
                "properties": {
                    "collection": {"type": "string"},
                    "ids": {"type": "array", "items": {"type": "string"}},
                    "documents": {"type": "array", "items": {"type": "string"}},
                    "force": {"type": "boolean"}
                },
                "required": ["collection", "ids", "documents"]
            }),
        ),
        tool_descriptor(
            "query",
            "Sök i samlingen med semantisk sökning, automatisk sökexpansion (middleware) och reranking",
            json!({
                "type": "object",
                "properties": {
                    "collection": {"type": "string"},
                    "query": {"type": "string"},
                    "top_k": {"type": "integer"}
                },
                "required": ["collection", "query"]
            }),
        ),
        tool_descriptor(
            "list_collections",
            "Lista alla samlingar i databasen",
            json!({ "type": "object", "properties": {} }),
        ),
        tool_descriptor(
            "delete_documents",
            "Ta bort ett eller flera indexerade dokument från en samling. Tom lista = rensa hela samlingen.",
            json!({
                "type": "object",
                "properties": {
                    "collection": {"type": "string"},
                    "ids": {"type": "array", "items": {"type": "string"}}
                },
                "required": ["collection", "ids"]
            }),
        ),
        tool_descriptor(
            "delete_collection",
            "Ta bort en hel samling inklusive alla dokument, chunks och metadata.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        ),
    ]
}

fn tool_descriptor(name: &str, description: &str, input_schema: serde_json::Value) -> serde_json::Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

pub async fn call_tool(
    name: &str,
    args: Value,
    cfg: &Config,
    db: &Arc<Mutex<Db>>,
    client: &reqwest::Client,
) -> Result<String> {
    match name {
        "create_collection" => {
            let args: create_collection::CreateCollectionArgs = serde_json::from_value(args)?;
            create_collection::create_collection(db, args).await
        }
        "ingest_file" => {
            let args: ingest_file::IngestFileArgs = serde_json::from_value(args)?;
            ingest_file::ingest_file(db, cfg, client, args).await
        }
        "ingest_directory" => {
            let args: ingest_directory::IngestDirectoryArgs = serde_json::from_value(args)?;
            ingest_directory::ingest_directory(db, cfg, client, args).await
        }
        "add_documents" => {
            let args: add_documents::AddDocumentsArgs = serde_json::from_value(args)?;
            add_documents::add_documents(db, cfg, client, args).await
        }
        "query" => {
            let args: query::QueryArgs = serde_json::from_value(args)?;
            query::query(db, cfg, client, args).await
        }
        "list_collections" => {
            list_collections::list_collections(db).await
        }
        "delete_documents" => {
            let args: delete_documents::DeleteDocumentsArgs = serde_json::from_value(args)?;
            delete_documents::delete_documents(db, args).await
        }
        "delete_collection" => {
            let args: delete_collection::DeleteCollectionArgs = serde_json::from_value(args)?;
            delete_collection::delete_collection(db, args).await
        }
        _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
    }
}