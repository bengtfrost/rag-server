use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::db::Db;

#[derive(Debug, Deserialize)]
pub struct CreateCollectionArgs {
    pub name: String,
}

pub async fn create_collection(
    db: &Arc<Mutex<Db>>,
    args: CreateCollectionArgs,
) -> anyhow::Result<String> {
    let db = db.lock().await;
    db.insert_collection(&args.name)?;
    Ok(format!("Samlingen '{}' är nu redo.", args.name))
}