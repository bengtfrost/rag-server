use anyhow::Result;
use rusqlite::{Connection, params};
use std::env;

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Hämta sökvägen till tillägget från miljövariabeln
        let ext_path = env::var("SQLITE_VEC_PATH").unwrap_or_else(|_| "vec0".to_string());

        unsafe {
            conn.load_extension_enable()?;
            conn.load_extension(ext_path, Some("sqlite3_vec_init"))?;
            conn.load_extension_disable()?;
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS collections (name TEXT PRIMARY KEY)",
            [],
        )?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS docs (
                id TEXT PRIMARY KEY, 
                collection TEXT, 
                text TEXT, 
                parent_id TEXT, 
                chunk_index INTEGER
            )",
            [],
        )?;

        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS vectors USING vec0(
                id TEXT PRIMARY KEY, 
                collection TEXT, 
                embedding FLOAT[1024]
            )",
            [],
        )?;

        Ok(Self { conn })
    }

    // ===== COLLECTION METODER =====

    pub fn add_collection(&self, name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO collections (name) VALUES (?)",
            [name],
        )?;
        Ok(())
    }

    pub fn insert_collection(&self, name: &str) -> Result<()> {
        self.add_collection(name) // alias
    }

    pub fn collection_exists(&self, name: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM collections WHERE name = ?",
            [name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn get_collection_stats(&self, name: &str) -> Result<(i64, i64)> {
        let doc_count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT parent_id) FROM docs WHERE collection = ?",
            [name],
            |row| row.get(0),
        )?;
        let chunk_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM docs WHERE collection = ?",
            [name],
            |row| row.get(0),
        )?;
        Ok((doc_count, chunk_count))
    }

    pub fn delete_collection(&self, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM docs WHERE collection = ?", [name])?;
        self.conn
            .execute("DELETE FROM vectors WHERE collection = ?", [name])?;
        self.conn
            .execute("DELETE FROM collections WHERE name = ?", [name])?;
        Ok(())
    }

    pub fn clear_collection(&self, name: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM docs WHERE collection = ?", [name])?;
        self.conn
            .execute("DELETE FROM vectors WHERE collection = ?", [name])?;
        Ok(())
    }

    pub fn list_collections(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.name, COUNT(DISTINCT d.parent_id) 
             FROM collections c 
             LEFT JOIN docs d ON c.name = d.collection 
             GROUP BY c.name",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    // ===== DOKUMENT METODER =====

    pub fn doc_exists(&self, collection: &str, doc_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM docs WHERE collection = ? AND parent_id = ?",
            params![collection, doc_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn delete_documents(&self, collection: &str, doc_ids: &[String]) -> Result<()> {
        for doc_id in doc_ids {
            self.conn.execute(
                "DELETE FROM docs WHERE collection = ? AND parent_id = ?",
                params![collection, doc_id],
            )?;
            self.conn.execute(
                "DELETE FROM vectors WHERE collection = ? AND id LIKE ?",
                params![collection, format!("{}%", doc_id)],
            )?;
        }
        Ok(())
    }

    // ===== CHUNK METODER =====

    // OBS: tar ägande av chunks och embs (inte referenser)
    pub fn insert_chunks(
        &self,
        coll: &str,
        parent_id: &str,
        chunks: Vec<String>,
        embs: Vec<Vec<f32>>,
    ) -> Result<()> {
        for (i, (text, emb)) in chunks.into_iter().zip(embs.into_iter()).enumerate() {
            let chunk_id = format!("{}_ch{}", parent_id, i);
            let emb_json = serde_json::to_string(&emb)?;

            self.conn.execute(
                "INSERT OR REPLACE INTO docs (id, collection, text, parent_id, chunk_index) VALUES (?1, ?2, ?3, ?4, ?5)", 
                params![chunk_id, coll, text, parent_id, i]
            )?;

            self.conn.execute(
                "INSERT OR REPLACE INTO vectors (id, collection, embedding) VALUES (?1, ?2, ?3)",
                params![chunk_id, coll, emb_json],
            )?;
        }
        Ok(())
    }

    pub fn replace_chunks_batch(
        &self,
        collection: &str,
        data: &[(String, Vec<String>, Vec<Vec<f32>>)],
    ) -> Result<()> {
        for (doc_id, chunks, embs) in data {
            // Ta bort gamla
            self.conn.execute(
                "DELETE FROM docs WHERE collection = ? AND parent_id = ?",
                params![collection, doc_id],
            )?;
            self.conn.execute(
                "DELETE FROM vectors WHERE collection = ? AND id LIKE ?",
                params![collection, format!("{}%", doc_id)],
            )?;
            // Lägg till nya (klona för att äga)
            self.insert_chunks(collection, doc_id, chunks.clone(), embs.clone())?;
        }
        Ok(())
    }

    pub fn get_chunk_texts(&self, chunk_ids: &[String]) -> Result<Vec<(String, String, String)>> {
        if chunk_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = chunk_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT id, text, parent_id FROM docs WHERE id IN ({})",
            placeholders
        );
        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(chunk_ids.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ===== SÖK METODER =====

    pub fn search(
        &self,
        coll: &str,
        emb: Vec<f32>,
        k: usize,
    ) -> Result<Vec<(String, String, f32)>> {
        let emb_json = serde_json::to_string(&emb)?;

        let mut stmt = self.conn.prepare(
            "SELECT v.id, d.text, v.distance 
             FROM vectors v 
             JOIN docs d ON v.id = d.id 
             WHERE v.collection = ?1 AND v.embedding MATCH ?2 AND k = ?3",
        )?;

        let rows = stmt.query_map(params![coll, emb_json, k], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }
}
