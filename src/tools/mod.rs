use anyhow::Result;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::db::Db;

pub mod add_documents;
pub mod create_collection;
pub mod delete_collection;
pub mod delete_documents;
pub mod ingest_directory;
pub mod ingest_file;
pub mod list_collections;
pub mod query;

pub use create_collection::CreateCollectionArgs;
pub use ingest_directory::IngestDirectoryArgs;
pub use ingest_file::IngestFileArgs;
// pub use add_documents::AddDocumentsArgs; // Commented if unused
pub use delete_collection::DeleteCollectionArgs;
pub use delete_documents::DeleteDocumentsArgs;
pub use query::QueryArgs;

pub fn list_tools() -> Vec<serde_json::Value> {
    vec![
        tool_descriptor(
            "create_collection",
            "Create a new RAG collection",
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
            "Read and index a file directly from disk. Set force=true to force re-indexing.",
            json!({
                "type": "object",
                "properties": {
                    "collection": {"type": "string"},
                    "file_path": {"type": "string"},
                    "document_id": {"type": "string"},
                    "encoding": {"type": "string"},
                    "force": {"type": "boolean"},
                    "pipeline": {"type": "boolean", "description": "Use pipeline optimization"}
                },
                "required": ["collection", "file_path"]
            }),
        ),
        tool_descriptor(
            "ingest_directory",
            "Index all matching files in a directory using parallel processing.",
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
            "Index raw text strings directly.",
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
            "Search the collection using hybrid search (BM25 + vector) and reranking",
            json!({
                "type": "object",
                "properties": {
                    "collection": {"type": "string"},
                    "query": {"type": "string"},
                    "top_k": {"type": "integer", "default": 5},
                    "rerank_url": {"type": "string", "description": "Optional reranker URL"},
                    "hybrid": {"type": "boolean", "description": "Use hybrid search", "default": false},
                    "vector_weight": {"type": "number", "description": "Weight for vector score (0-1)", "default": 0.7},
                    "bm25_weight": {"type": "number", "description": "Weight for BM25 score (0-1)", "default": 0.3},
                    "no_cache": {"type": "boolean", "description": "Skip cache", "default": false}
                },
                "required": ["collection", "query"]
            }),
        ),
        tool_descriptor(
            "list_collections",
            "List all collections in the database",
            json!({ "type": "object", "properties": {} }),
        ),
        tool_descriptor(
            "delete_documents",
            "Delete one or more indexed documents from a collection. Empty list = clear the entire collection.",
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
            "Delete an entire collection including all documents, chunks, and metadata.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        ),
        tool_descriptor(
            "clear_cache",
            "Clear the query cache (TTL-based cache with LRU eviction)",
            json!({ "type": "object", "properties": {} }),
        ),
    ]
}

fn tool_descriptor(
    name: &str,
    description: &str,
    input_schema: serde_json::Value,
) -> serde_json::Value {
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
            let args: create_collection::CreateCollectionArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("Invalid arguments for 'create_collection'. Expected {{ name: string }}. Error: {}", e))?;
            create_collection::create_collection(db, args).await
        }
        "ingest_file" => {
            let args: ingest_file::IngestFileArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("Invalid arguments for 'ingest_file'. Expected fields: collection, file_path (optional: document_id, encoding, force, pipeline). Error: {}", e))?;
            ingest_file::ingest_file(db, cfg, client, args).await
        }
        "ingest_directory" => {
            let args: ingest_directory::IngestDirectoryArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("Invalid arguments for 'ingest_directory'. Expected fields: collection, directory_path (optional: file_extensions, encoding, force). Error: {}", e))?;
            ingest_directory::ingest_directory(db, cfg, client, args).await
        }
        "add_documents" => {
            let args: add_documents::AddDocumentsArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("Invalid arguments for 'add_documents'. Expected fields: collection, ids (array), documents (array), force (bool). Error: {}", e))?;
            add_documents::add_documents(db, cfg, client, args).await
        }
        "query" => {
            let args: query::QueryArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("Invalid arguments for 'query'. Expected fields: collection, query (optional: top_k, rerank_url, hybrid, vector_weight, bm25_weight, no_cache). Error: {}", e))?;
            query::query(db, cfg, client, args).await
        }
        "list_collections" => list_collections::list_collections(db).await,
        "delete_documents" => {
            let args: delete_documents::DeleteDocumentsArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("Invalid arguments for 'delete_documents'. Expected fields: collection, ids (array). Error: {}", e))?;
            delete_documents::delete_documents(db, args).await
        }
        "delete_collection" => {
            let args: delete_collection::DeleteCollectionArgs = serde_json::from_value(args)
                .map_err(|e| anyhow::anyhow!("Invalid arguments for 'delete_collection'. Expected {{ name: string }}. Error: {}", e))?;
            delete_collection::delete_collection(db, args).await
        }
        "clear_cache" => {
            crate::cache::QUERY_CACHE.clear();
            Ok("✓ Cache cleared. All cached queries have been removed.".to_string())
        }
        _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
    }
}
