use crate::db::Db;
use std::sync::Arc;
use tokio::sync::Mutex;

pub async fn list_collections(db: &Arc<Mutex<Db>>) -> anyhow::Result<String> {
    let db_guard = db.lock().await;
    // Använd den befintliga metoden `list_collections` (inte `get_collections`)
    let collections = db_guard.list_collections()?;
    if collections.is_empty() {
        return Ok("Inga samlingar än.".to_string());
    }
    let lines: Vec<String> = collections
        .iter()
        .map(|(name, count)| format!("• {}: {} dokument", name, count))
        .collect();
    Ok(format!("Databasens samlingar:\n{}", lines.join("\n")))
}

