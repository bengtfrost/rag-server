use rusqlite::{Connection, params, ToSql};
use tracing::debug;

pub struct Db {
    pub conn: Connection,
}

impl Db {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        Ok(Self { conn })
    }

    pub fn init(&mut self, vec_path: &str) -> anyhow::Result<()> {
        // sqlite-vec extension load requires unsafe
        unsafe {
            self.conn.load_extension_enable()?;
            self.conn.load_extension(vec_path, None)?;
        }

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS collections (name TEXT PRIMARY KEY)",
            [],
        )?;
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS docs (
                id          TEXT PRIMARY KEY,
                collection  TEXT,
                text        TEXT,
                parent_id   TEXT,
                chunk_index INTEGER
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vectors USING vec0(
                id         TEXT PRIMARY KEY,
                collection TEXT,
                embedding  FLOAT[1024]
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_docs_collection ON docs(collection)",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_docs_parent ON docs(parent_id, collection)",
            [],
        )?;

        if let Some(p) = self.conn.path() {
            debug!("Database initialized at {}", p);
        }
        Ok(())
    }

    pub fn collection_exists(&self, name: &str) -> anyhow::Result<bool> {
        let mut stmt = self.conn.prepare("SELECT 1 FROM collections WHERE name = ?")?;
        let exists = stmt.exists([name])?;
        Ok(exists)
    }

    pub fn insert_collection(&self, name: &str) -> anyhow::Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO collections (name) VALUES (?)", [name])?;
        Ok(())
    }

    pub fn doc_exists(&self, collection: &str, parent_id: &str) -> anyhow::Result<bool> {
        let mut stmt = self.conn.prepare(
            "SELECT 1 FROM docs WHERE parent_id = ? AND collection = ? LIMIT 1"
        )?;
        let exists = stmt.exists([parent_id, collection])?;
        Ok(exists)
    }

    pub fn delete_documents(&self, collection: &str, parent_ids: &[String]) -> anyhow::Result<usize> {
        let mut deleted = 0;
        for pid in parent_ids {
            self.conn.execute(
                "DELETE FROM vectors WHERE id LIKE ?",
                [&format!("{}_ch%", pid)],
            )?;
            let affected = self.conn.execute(
                "DELETE FROM docs WHERE parent_id = ? AND collection = ?",
                [pid, collection],
            )?;
            deleted += affected;
        }
        Ok(deleted)
    }

    pub fn clear_collection(&self, collection: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM vectors WHERE collection = ?", [collection])?;
        self.conn.execute("DELETE FROM docs WHERE collection = ?", [collection])?;
        Ok(())
    }

    pub fn delete_collection(&self, name: &str) -> anyhow::Result<()> {
        self.conn.execute("DELETE FROM vectors WHERE collection = ?", [name])?;
        self.conn.execute("DELETE FROM docs WHERE collection = ?", [name])?;
        self.conn.execute("DELETE FROM collections WHERE name = ?", [name])?;
        Ok(())
    }

    pub fn get_collections(&self) -> anyhow::Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.name, COUNT(DISTINCT d.parent_id) FROM collections c
             LEFT JOIN docs d ON c.name = d.collection
             GROUP BY c.name"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn insert_chunks(
        &self,
        collection: &str,
        parent_id: &str,
        chunks: &[String],
        embeddings: &[Vec<f32>],
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM vectors WHERE id LIKE ?",
            [&format!("{}_ch%", parent_id)],
        )?;
        self.conn.execute(
            "DELETE FROM docs WHERE parent_id = ? AND collection = ?",
            [parent_id, collection],
        )?;

        for (idx, (chunk_text, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            let chunk_id = format!("{}_ch{}", parent_id, idx);
            let emb_json = serde_json::to_string(emb)?;
            self.conn.execute(
                "INSERT INTO vectors (id, collection, embedding) VALUES (?, ?, ?)",
                [&chunk_id, collection, &emb_json],
            )?;
            self.conn.execute(
                "INSERT INTO docs (id, collection, text, parent_id, chunk_index) VALUES (?, ?, ?, ?, ?)",
                params![&chunk_id, collection, chunk_text, parent_id, idx],
            )?;
        }
        Ok(())
    }

    pub fn ann_search(&self, collection: &str, embedding: &[f32], k: usize) -> anyhow::Result<Vec<String>> {
        let emb_json = serde_json::to_string(embedding)?;
        let mut stmt = self.conn.prepare(
            "SELECT id FROM vectors WHERE collection = ? AND embedding MATCH ? AND k = ?"
        )?;
        let rows = stmt.query_map([collection, &emb_json, &k.to_string()], |row| {
            row.get::<_, String>(0)
        })?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    pub fn get_chunk_texts(&self, ids: &[String]) -> anyhow::Result<Vec<(String, String, String)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id, text, parent_id FROM docs WHERE id IN ({})", placeholders);

        let params: Vec<&dyn ToSql> = ids.iter().map(|s| s as &dyn ToSql).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_collection_stats(&self, collection: &str) -> anyhow::Result<(i64, i64)> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(DISTINCT parent_id), COUNT(*) FROM docs WHERE collection = ?"
        )?;
        let mut rows = stmt.query([collection])?;
        if let Some(row) = rows.next()? {
            let doc_count = row.get(0)?;
            let chunk_count = row.get(1)?;
            Ok((doc_count, chunk_count))
        } else {
            Ok((0, 0))
        }
    }
}