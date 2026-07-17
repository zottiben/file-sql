use std::collections::HashMap;
use std::sync::Once;

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension};
use tokio_rusqlite::Connection;

use crate::config::Config;
use crate::model::{Category, FileRecord, IndexedFile, SearchHit, SearchQuery};
use crate::storage::{now_unix, recency_bonus, rrf, Storage};
use crate::{Error, Result};

/// Over-fetch factor per search leg before rank fusion.
const OVERFETCH: usize = 5;
/// Max characters returned per snippet, to keep results token-cheap.
const SNIPPET_CHARS: usize = 800;

/// SQLite backend: `sqlite-vec` for KNN, FTS5 (trigram tokenizer) for text.
/// Zero-setup and single-file - the default, per-repo backend.
pub struct SqliteStore {
    conn: Connection,
    dims: usize,
    model: String,
}

fn register_vec_extension() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // sqlite-vec ships its init fn with its own sqlite header types, so we
        // transmute to the entry-point shape rusqlite's ffi expects. Registering
        // as an auto-extension applies it to every later connection.
        type VecInit = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), VecInit>(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }
    });
}

fn map_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Storage(e.to_string())
}

fn truncate_snippet(s: &str) -> String {
    if s.len() <= SNIPPET_CHARS {
        return s.to_string();
    }
    let mut end = SNIPPET_CHARS;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Quote arbitrary user text as a single FTS5 string token so query syntax
/// characters can't break the MATCH expression.
fn fts_quote(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

impl SqliteStore {
    pub async fn open(config: &Config) -> Result<Self> {
        register_vec_extension();
        let path = config.storage.sqlite_path.clone();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path).await.map_err(map_err)?;
        Ok(SqliteStore {
            conn,
            dims: config.embedding.dims,
            model: config.embedding.index_model_key(),
        })
    }
}

#[async_trait]
impl Storage for SqliteStore {
    async fn migrate(&self) -> Result<()> {
        let dims = self.dims;
        let schema = format!(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS files (
                 id               INTEGER PRIMARY KEY,
                 path             TEXT NOT NULL UNIQUE,
                 category         TEXT NOT NULL,
                 language         TEXT,
                 size_bytes       INTEGER NOT NULL,
                 sha256           TEXT NOT NULL,
                 mtime            INTEGER NOT NULL,
                 git_last_commit  INTEGER,
                 git_last_author  TEXT,
                 git_commit_count INTEGER,
                 summary          TEXT
             );
             CREATE TABLE IF NOT EXISTS symbols (
                 id         INTEGER PRIMARY KEY,
                 file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 name       TEXT NOT NULL,
                 kind       TEXT NOT NULL,
                 start_line INTEGER NOT NULL,
                 end_line   INTEGER NOT NULL,
                 signature  TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
             CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
             CREATE TABLE IF NOT EXISTS chunks (
                 id         INTEGER PRIMARY KEY,
                 file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 symbol_id  INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
                 start_line INTEGER NOT NULL,
                 end_line   INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_chunks_file ON chunks(file_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS chunk_fts USING fts5(
                 content,
                 tokenize='trigram'
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS chunk_vectors USING vec0(
                 chunk_id INTEGER PRIMARY KEY,
                 embedding FLOAT[{dims}] distance_metric=cosine
             );"
        );
        self.conn
            .call(move |conn| -> rusqlite::Result<()> {
                conn.execute_batch(&schema)?;
                Ok(())
            })
            .await
            .map_err(map_err)?;

        // Guard: refuse to mix embeddings from different models/dims.
        let dims_str = dims.to_string();
        let model = self.model.clone();
        let existing: Option<(String, String)> = self
            .conn
            .call(|conn| -> rusqlite::Result<Option<(String, String)>> {
                let d: Option<String> = conn
                    .query_row(
                        "SELECT value FROM meta WHERE key='embedding_dims'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?;
                let m: Option<String> = conn
                    .query_row(
                        "SELECT value FROM meta WHERE key='embedding_model'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?;
                Ok(d.zip(m))
            })
            .await
            .map_err(map_err)?;

        match existing {
            Some((d, m)) if d != dims_str || m != model => Err(Error::Storage(format!(
                "index was built with vectorization={m} dims={d} but config has vectorization={model} dims={dims_str}; delete the old SQLite index file before rebuilding"
            ))),
            Some(_) => Ok(()),
            None => {
                self.conn
                    .call(move |conn| -> rusqlite::Result<()> {
                        conn.execute(
                            "INSERT INTO meta(key, value) VALUES('embedding_dims', ?1), ('embedding_model', ?2)",
                            params![dims_str, model],
                        )?;
                        Ok(())
                    })
                    .await
                    .map_err(map_err)?;
                Ok(())
            }
        }
    }

    async fn file_hash(&self, path: &str) -> Result<Option<String>> {
        let path = path.to_string();
        self.conn
            .call(move |conn| {
                conn.query_row(
                    "SELECT sha256 FROM files WHERE path = ?1",
                    params![path],
                    |r| r.get::<_, String>(0),
                )
                .optional()
            })
            .await
            .map_err(map_err)
    }

    async fn upsert_file(&self, indexed: &IndexedFile) -> Result<()> {
        for c in &indexed.chunks {
            if c.embedding.len() != self.dims {
                return Err(Error::Storage(format!(
                    "chunk embedding has {} dims, expected {}",
                    c.embedding.len(),
                    self.dims
                )));
            }
        }
        let indexed = indexed.clone();
        self.conn
            .call(move |conn| -> rusqlite::Result<()> {
                let tx = conn.transaction()?;
                purge_file(&tx, &indexed.file.path)?;

                let f = &indexed.file;
                tx.execute(
                    "INSERT INTO files
                       (path, category, language, size_bytes, sha256, mtime,
                        git_last_commit, git_last_author, git_commit_count, summary)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        f.path,
                        f.category.as_str(),
                        f.language,
                        f.size_bytes as i64,
                        f.sha256,
                        f.mtime,
                        f.git_last_commit,
                        f.git_last_author,
                        f.git_commit_count,
                        f.summary,
                    ],
                )?;
                let file_id = tx.last_insert_rowid();

                let mut symbol_ids = Vec::with_capacity(indexed.symbols.len());
                for s in &indexed.symbols {
                    tx.execute(
                        "INSERT INTO symbols(file_id, name, kind, start_line, end_line, signature)
                         VALUES (?1,?2,?3,?4,?5,?6)",
                        params![
                            file_id,
                            s.name,
                            s.kind.as_str(),
                            s.start_line as i64,
                            s.end_line as i64,
                            s.signature,
                        ],
                    )?;
                    symbol_ids.push(tx.last_insert_rowid());
                }

                for c in &indexed.chunks {
                    let symbol_id = c.symbol_index.and_then(|i| symbol_ids.get(i).copied());
                    tx.execute(
                        "INSERT INTO chunks(file_id, symbol_id, start_line, end_line)
                         VALUES (?1,?2,?3,?4)",
                        params![file_id, symbol_id, c.start_line as i64, c.end_line as i64],
                    )?;
                    let chunk_id = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO chunk_fts(rowid, content) VALUES (?1, ?2)",
                        params![chunk_id, c.content],
                    )?;
                    let embedding = serde_json::to_string(&c.embedding)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                    tx.execute(
                        "INSERT INTO chunk_vectors(chunk_id, embedding) VALUES (?1, ?2)",
                        params![chunk_id, embedding],
                    )?;
                }

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(map_err)
    }

    async fn delete_file(&self, path: &str) -> Result<()> {
        let path = path.to_string();
        self.conn
            .call(move |conn| -> rusqlite::Result<()> {
                let tx = conn.transaction()?;
                purge_file(&tx, &path)?;
                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(map_err)
    }

    async fn all_paths(&self) -> Result<Vec<String>> {
        self.conn
            .call(|conn| {
                let mut stmt = conn.prepare("SELECT path FROM files")?;
                let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect()
            })
            .await
            .map_err(map_err)
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        let query = query.clone();
        let now = now_unix();
        self.conn
            .call(move |conn| run_search(conn, &query, now))
            .await
            .map_err(map_err)
    }

    async fn find_symbol(&self, name: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let name = name.to_string();
        let like = format!("%{name}%");
        self.conn
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT s.name, s.start_line, s.end_line, s.signature,
                            f.path, f.category, f.language, f.summary
                     FROM symbols s
                     JOIN files f ON f.id = s.file_id
                     WHERE s.name = ?1 OR s.name LIKE ?2
                     ORDER BY CASE WHEN s.name = ?1 THEN 0 ELSE 1 END, length(s.name)
                     LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![name, like, limit as i64], |r| {
                    let sym_name: String = r.get(0)?;
                    let signature: Option<String> = r.get(3)?;
                    let category: String = r.get(5)?;
                    let exact = sym_name == name;
                    Ok(SearchHit {
                        path: r.get(4)?,
                        category: Category::from_db(&category),
                        language: r.get(6)?,
                        summary: r.get(7)?,
                        score: if exact { 1.0 } else { 0.5 },
                        start_line: r.get::<_, i64>(1)? as u32,
                        end_line: r.get::<_, i64>(2)? as u32,
                        snippet: signature.clone().unwrap_or_else(|| sym_name.clone()),
                        symbol: Some(sym_name),
                    })
                })?;
                rows.collect()
            })
            .await
            .map_err(map_err)
    }

    async fn recently_changed(&self, limit: usize) -> Result<Vec<FileRecord>> {
        self.conn
            .call(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT path, category, language, size_bytes, sha256, mtime,
                            git_last_commit, git_last_author, git_commit_count, summary
                     FROM files
                     ORDER BY COALESCE(git_last_commit, mtime) DESC
                     LIMIT ?1",
                )?;
                let rows = stmt.query_map(params![limit as i64], |r| {
                    let category: String = r.get(1)?;
                    Ok(FileRecord {
                        path: r.get(0)?,
                        category: Category::from_db(&category),
                        language: r.get(2)?,
                        size_bytes: r.get::<_, i64>(3)? as u64,
                        sha256: r.get(4)?,
                        mtime: r.get(5)?,
                        git_last_commit: r.get(6)?,
                        git_last_author: r.get(7)?,
                        git_commit_count: r.get(8)?,
                        summary: r.get(9)?,
                    })
                })?;
                rows.collect()
            })
            .await
            .map_err(map_err)
    }
}

/// Remove a file and its FTS/vector rows (which foreign keys can't cascade into
/// virtual tables). Must run inside a transaction.
fn purge_file(tx: &rusqlite::Transaction<'_>, path: &str) -> rusqlite::Result<()> {
    let file_id: Option<i64> = tx
        .query_row("SELECT id FROM files WHERE path = ?1", params![path], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(file_id) = file_id else {
        return Ok(());
    };
    let chunk_ids: Vec<i64> = {
        let mut stmt = tx.prepare("SELECT id FROM chunks WHERE file_id = ?1")?;
        let rows = stmt.query_map(params![file_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for id in chunk_ids {
        tx.execute("DELETE FROM chunk_fts WHERE rowid = ?1", params![id])?;
        tx.execute("DELETE FROM chunk_vectors WHERE chunk_id = ?1", params![id])?;
    }
    tx.execute("DELETE FROM files WHERE id = ?1", params![file_id])?;
    Ok(())
}

fn run_search(
    conn: &rusqlite::Connection,
    query: &SearchQuery,
    now: i64,
) -> rusqlite::Result<Vec<SearchHit>> {
    let over = (query.limit.max(1) * OVERFETCH).max(20) as i64;

    let mut vec_ids: Vec<i64> = Vec::new();
    if !query.embedding.is_empty() {
        let embedding = serde_json::to_string(&query.embedding)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        let mut stmt = conn.prepare(
            "SELECT chunk_id FROM chunk_vectors
             WHERE embedding MATCH ?1 AND k = ?2
             ORDER BY distance",
        )?;
        let rows = stmt.query_map(params![embedding, over], |r| r.get(0))?;
        vec_ids = rows.collect::<rusqlite::Result<_>>()?;
    }

    let mut text_ids: Vec<i64> = Vec::new();
    if query.text.trim().chars().count() >= 3 {
        let mut stmt = conn.prepare(
            "SELECT rowid FROM chunk_fts
             WHERE chunk_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_quote(&query.text), over], |r| r.get(0))?;
        text_ids = rows.collect::<rusqlite::Result<_>>()?;
    }

    let fused = rrf(&[vec_ids, text_ids]);
    if fused.is_empty() {
        return Ok(Vec::new());
    }

    let mut meta_stmt = conn.prepare(
        "SELECT c.file_id, c.start_line, c.end_line, s.name,
                f.path, f.category, f.language, f.summary,
                f.git_last_commit, f.mtime, cf.content
         FROM chunks c
         JOIN files f ON f.id = c.file_id
         LEFT JOIN symbols s ON s.id = c.symbol_id
         JOIN chunk_fts cf ON cf.rowid = c.id
         WHERE c.id = ?1",
    )?;

    // Keep the best-scoring chunk per file.
    let mut best: HashMap<i64, (f64, SearchHit)> = HashMap::new();
    for (chunk_id, base) in fused {
        let row = meta_stmt
            .query_row(params![chunk_id], |r| {
                let category: String = r.get(5)?;
                Ok((
                    r.get::<_, i64>(0)?,            // file_id
                    r.get::<_, i64>(1)? as u32,     // start_line
                    r.get::<_, i64>(2)? as u32,     // end_line
                    r.get::<_, Option<String>>(3)?, // symbol
                    r.get::<_, String>(4)?,         // path
                    category,
                    r.get::<_, Option<String>>(6)?, // language
                    r.get::<_, Option<String>>(7)?, // summary
                    r.get::<_, Option<i64>>(8)?,    // git_last_commit
                    r.get::<_, i64>(9)?,            // mtime
                    r.get::<_, String>(10)?,        // content
                ))
            })
            .optional()?;
        let Some((
            file_id,
            start,
            end,
            symbol,
            path,
            category,
            language,
            summary,
            git,
            mtime,
            content,
        )) = row
        else {
            continue;
        };

        let category = Category::from_db(&category);
        if !query.categories.is_empty() && !query.categories.contains(&category) {
            continue;
        }
        if !query.path_prefixes.is_empty()
            && !query.path_prefixes.iter().any(|p| path.starts_with(p))
        {
            continue;
        }

        let score = base + recency_bonus(git, mtime, now);
        let entry = best.entry(file_id).or_insert((f64::MIN, dummy_hit()));
        if score > entry.0 {
            *entry = (
                score,
                SearchHit {
                    path,
                    category,
                    language,
                    summary,
                    score: score as f32,
                    start_line: start,
                    end_line: end,
                    symbol,
                    snippet: truncate_snippet(&content),
                },
            );
        }
    }

    let mut hits: Vec<SearchHit> = best.into_values().map(|(_, hit)| hit).collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(query.limit);
    Ok(hits)
}

fn dummy_hit() -> SearchHit {
    SearchHit {
        path: String::new(),
        category: Category::Other,
        language: None,
        summary: None,
        score: 0.0,
        start_line: 0,
        end_line: 0,
        symbol: None,
        snippet: String::new(),
    }
}
