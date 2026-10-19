use anyhow::Result;
use rusqlite::{Connection, params};
use std::collections::HashMap;

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        let ext_path = std::env::var("SQLITE_VEC_PATH").unwrap_or_else(|_| "vec0".to_string());

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
            "CREATE INDEX IF NOT EXISTS idx_docs_parent ON docs(collection, parent_id)",
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

        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(
                id UNINDEXED,
                collection UNINDEXED,
                text,
                content=docs,
                content_rowid=rowid
            )",
            [],
        )?;

        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS docs_ai AFTER INSERT ON docs BEGIN
                INSERT INTO docs_fts(rowid, id, collection, text)
                VALUES (new.rowid, new.id, new.collection, new.text);
            END",
            [],
        )?;
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS docs_ad AFTER DELETE ON docs BEGIN
                DELETE FROM docs_fts WHERE rowid = old.rowid;
            END",
            [],
        )?;
        conn.execute(
            "CREATE TRIGGER IF NOT EXISTS docs_au AFTER UPDATE ON docs BEGIN
                DELETE FROM docs_fts WHERE rowid = old.rowid;
                INSERT INTO docs_fts(rowid, id, collection, text)
                VALUES (new.rowid, new.id, new.collection, new.text);
            END",
            [],
        )?;

        Ok(Self { conn })
    }

    // ===== COLLECTION METHODS =====

    pub fn add_collection(&self, name: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO collections (name) VALUES (?)",
            [name],
        )?;
        Ok(())
    }

    pub fn insert_collection(&self, name: &str) -> Result<()> {
        self.add_collection(name)
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

    // ===== DOCUMENT METHODS =====

    pub fn doc_exists(&self, collection: &str, doc_id: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM docs WHERE collection = ? AND parent_id = ?",
            params![collection, doc_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Batch existence check — avoids one DB round-trip per id.
    pub fn existing_doc_ids(
        &self,
        collection: &str,
        ids: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        use std::collections::HashSet;
        if ids.is_empty() {
            return Ok(HashSet::new());
        }
        let ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT DISTINCT parent_id FROM docs WHERE collection = ? AND parent_id IN ({})",
            ph
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(ids.len() + 1);
        params_vec.push(&collection);
        for id in ids {
            params_vec.push(id);
        }
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), |r| {
            r.get::<_, String>(0)
        })?;
        let mut set = HashSet::new();
        for r in rows {
            set.insert(r?);
        }
        Ok(set)
    }

    pub fn delete_documents(&self, collection: &str, doc_ids: &[String]) -> Result<()> {
        for doc_id in doc_ids {
            delete_doc_chunks(&self.conn, collection, doc_id)?;
        }
        Ok(())
    }

    // ===== CHUNK METHODS =====

    pub fn insert_chunks(
        &mut self,
        coll: &str,
        parent_id: &str,
        chunks: &[String],
        embs: &[Vec<f32>],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        Self::insert_chunks_inner(&tx, coll, parent_id, chunks, embs)?;
        tx.commit()?;
        Ok(())
    }

    pub fn replace_chunks_batch(
        &mut self,
        collection: &str,
        data: &[(String, Vec<String>, Vec<Vec<f32>>)],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (doc_id, chunks, embs) in data {
            Self::insert_chunks_inner(&tx, collection, doc_id, chunks, embs)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn insert_chunks_inner(
        conn: &rusqlite::Connection,
        collection: &str,
        parent_id: &str,
        chunks: &[String],
        embs: &[Vec<f32>],
    ) -> Result<()> {
        if chunks.len() != embs.len() {
            anyhow::bail!(
                "chunks/embeddings length mismatch: {} vs {}",
                chunks.len(),
                embs.len()
            );
        }

        // Delete *all* existing chunks for this parent first so re-indexing with a
        // smaller chunk count cannot leave orphans behind.
        delete_doc_chunks(conn, collection, parent_id)?;

        for (i, (text, emb)) in chunks.iter().zip(embs.iter()).enumerate() {
            let chunk_id = format!("{}_ch{}", parent_id, i);
            let emb_json = serde_json::to_string(emb)?;
            conn.execute(
                "INSERT INTO docs (id, collection, text, parent_id, chunk_index) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![chunk_id, collection, text, parent_id, i as i64],
            )?;
            conn.execute(
                "INSERT INTO vectors (id, collection, embedding) VALUES (?1, ?2, ?3)",
                params![chunk_id, collection, emb_json],
            )?;
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

    // ===== SEARCH METHODS =====

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

        let rows = stmt.query_map(params![coll, emb_json, k as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    pub fn bm25_search(&self, coll: &str, query: &str, k: usize) -> Result<Vec<(String, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, bm25(docs_fts) as score
             FROM docs_fts
             WHERE docs_fts MATCH ?1 AND collection = ?2
             ORDER BY bm25(docs_fts)
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![query, coll, k as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    // ===== HYBRID SEARCH =====

    pub fn hybrid_search(
        &self,
        coll: &str,
        emb: Vec<f32>,
        query_text: &str,
        k: usize,
        vector_weight: f64,
        bm25_weight: f64,
    ) -> Result<Vec<(String, String, f64)>> {
        let vector_results = self.search(coll, emb, k * 2)?;
        let bm25_results = self.bm25_search(coll, query_text, k * 2)?;

        let vector_scores: Vec<(String, f64)> = vector_results
            .iter()
            .map(|(id, _, score)| (id.clone(), *score as f64))
            .collect();
        let vector_norm = Self::normalize_scores(&vector_scores);
        let bm25_norm = Self::normalize_scores(&bm25_results);

        let mut combined: HashMap<String, (f64, f64)> = HashMap::new();
        for (id, score) in vector_norm {
            combined.entry(id).or_insert((0.0, 0.0)).0 = score;
        }
        for (id, score) in bm25_norm {
            combined.entry(id).or_insert((0.0, 0.0)).1 = score;
        }

        let mut scored_ids: Vec<(String, f64)> = combined
            .into_iter()
            .map(|(id, (v_score, b_score))| (id, v_score * vector_weight + b_score * bm25_weight))
            .collect();

        scored_ids.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored_ids.truncate(k);

        let ids: Vec<String> = scored_ids.iter().map(|(id, _)| id.clone()).collect();
        let chunk_texts = self.get_chunk_texts(&ids)?;

        let mut result = Vec::new();
        for (id, combined_score) in scored_ids {
            if let Some((_, text, parent)) =
                chunk_texts.iter().find(|(chunk_id, _, _)| chunk_id == &id)
            {
                let distance = 1.0 - combined_score.min(1.0);
                result.push((id, text.clone(), parent.clone(), distance));
            }
        }

        result.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
        Ok(result
            .into_iter()
            .map(|(id, text, _, dist)| (id, text, dist))
            .collect())
    }

    fn normalize_scores(scores: &[(String, f64)]) -> Vec<(String, f64)> {
        if scores.is_empty() {
            return Vec::new();
        }
        let max_score = scores
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_score = scores.iter().map(|(_, s)| *s).fold(f64::INFINITY, f64::min);
        let range = max_score - min_score;
        if range == 0.0 {
            return scores.iter().map(|(id, _)| (id.clone(), 0.5)).collect();
        }
        scores
            .iter()
            .map(|(id, s)| (id.clone(), (s - min_score) / range))
            .collect()
    }
}

/// Delete every chunk belonging to `doc_id` in `collection`.
///
/// Uses exact chunk IDs derived from `docs.parent_id` rather than `LIKE 'doc%'`
/// so a doc named `test` cannot accidentally delete a doc named `testing`.
fn delete_doc_chunks(conn: &rusqlite::Connection, collection: &str, doc_id: &str) -> Result<()> {
    let old_ids: Vec<String> = {
        let mut stmt =
            conn.prepare("SELECT id FROM docs WHERE collection = ? AND parent_id = ?")?;
        let rows = stmt.query_map(params![collection, doc_id], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    if !old_ids.is_empty() {
        let ph = old_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM vectors WHERE id IN ({})", ph);
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            old_ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        conn.execute(&sql, rusqlite::params_from_iter(param_refs))?;
    }
    conn.execute(
        "DELETE FROM docs WHERE collection = ? AND parent_id = ?",
        params![collection, doc_id],
    )?;
    Ok(())
}
