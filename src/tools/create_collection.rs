use crate::db::Db;
use clap::Args;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Deserialize, Args)]
pub struct CreateCollectionArgs {
    #[arg(short, long)]
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

