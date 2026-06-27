use crate::db::Db;
use clap::Args;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Deserialize, Args)]
pub struct DeleteDocumentsArgs {
    #[arg(short, long)]
    pub collection: String,
    #[arg(short, long, value_delimiter = ',')]
    pub ids: Vec<String>,
}

pub async fn delete_documents(
    db: &Arc<Mutex<Db>>,
    args: DeleteDocumentsArgs,
) -> anyhow::Result<String> {
    let db_guard = db.lock().await;

    if !db_guard.collection_exists(&args.collection)? {
        return Err(anyhow::anyhow!(
            "Fel: Samlingen '{}' finns inte.",
            args.collection
        ));
    }

    if args.ids.is_empty() {
        let (doc_count, _) = db_guard.get_collection_stats(&args.collection)?;
        db_guard.clear_collection(&args.collection)?;
        return Ok(format!(
            "✓ Samlingen '{}' är nu tom. {} dokument borttagna.",
            args.collection, doc_count
        ));
    }

    let mut existing = Vec::new();
    let mut missing = Vec::new();
    for pid in &args.ids {
        if db_guard.doc_exists(&args.collection, pid)? {
            existing.push(pid.clone());
        } else {
            missing.push(pid.clone());
        }
    }

    if !existing.is_empty() {
        db_guard.delete_documents(&args.collection, &existing)?;
    }

    let mut msg = format!(
        "✓ Tog bort {} dokument från samlingen '{}'.",
        existing.len(),
        args.collection
    );
    if !missing.is_empty() {
        msg.push_str(&format!(
            "\nVarning: Hittades inte och hoppades över: {}",
            missing.join(", ")
        ));
    }
    Ok(msg)
}
