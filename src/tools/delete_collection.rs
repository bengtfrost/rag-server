use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::db::Db;

#[derive(Debug, Deserialize)]
pub struct DeleteCollectionArgs {
    pub name: String,
}

pub async fn delete_collection(
    db: &Arc<Mutex<Db>>,
    args: DeleteCollectionArgs,
) -> anyhow::Result<String> {
    let db_guard = db.lock().await;

    if !db_guard.collection_exists(&args.name)? {
        return Err(anyhow::anyhow!("Fel: Samlingen '{}' finns inte.", args.name));
    }

    let (doc_count, chunk_count) = db_guard.get_collection_stats(&args.name)?;
    db_guard.delete_collection(&args.name)?;

    Ok(format!(
        "✓ Samlingen '{}' är borttagen. {} dokument och {} segment raderade.",
        args.name, doc_count, chunk_count
    ))
}