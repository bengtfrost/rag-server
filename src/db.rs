use anyhow::Result;
use rusqlite::{Connection, params};
use std::env;

pub struct VectorDB {
    conn: Connection,
}

impl VectorDB {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;

        // Hämta sökvägen till tillägget från miljövariabeln
        // Om den saknas försöker vi ladda den som "vec0" från systemets bibliotek
        let ext_path = env::var("SQLITE_VEC_PATH").unwrap_or_else(|_| "vec0".to_string());

        // Aktivera och ladda vec0 (v0.1.9+)
        unsafe {
            conn.load_extension_enable()?;
            // För vec0.so krävs ofta att man anger entry point "sqlite3_vec_init"
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

        // Skapa den virtuella vektortabellen (1024 dimensioner för BGE-M3)
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

    pub fn add_collection(&self, name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO collections (name) VALUES (?)",
            [name],
        )?;
        Ok(())
    }

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

    pub fn search(
        &self,
        coll: &str,
        emb: Vec<f32>,
        k: usize,
    ) -> Result<Vec<(String, String, f32)>> {
        let emb_json = serde_json::to_string(&emb)?;

        // Notera: 'distance' hämtas från den virtuella tabellen vec0
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
}
