use crate::db::Db;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn list_collections(db: &Arc<Mutex<Db>>) -> anyhow::Result<String> {
    let db_guard = db.lock().await;
    // Use the existing `list_collections` method (not `get_collections`)
    let collections = db_guard.list_collections()?;
    if collections.is_empty() {
        return Ok("No collections yet.".to_string());
    }
    let lines: Vec<String> = collections
        .iter()
        .map(|(name, count)| format!("• {}: {} documents", name, count))
        .collect();
    Ok(format!("Database collections:\n{}", lines.join("\n")))
}
