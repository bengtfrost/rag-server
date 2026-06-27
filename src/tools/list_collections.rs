use std::sync::Arc;
use tokio::sync::Mutex;
use crate::db::Db;

pub async fn list_collections(
    db: &Arc<Mutex<Db>>,
) -> anyhow::Result<String> {
    let db_guard = db.lock().await;
    let collections = db_guard.get_collections()?;
    if collections.is_empty() {
        return Ok("Inga samlingar än.".to_string());
    }
    let lines: Vec<String> = collections.iter()
        .map(|(name, count)| format!("• {}: {} dokument", name, count))
        .collect();
    Ok(format!("Databasens samlingar:\n{}", lines.join("\n")))
}