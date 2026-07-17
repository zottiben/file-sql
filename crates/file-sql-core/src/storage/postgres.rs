use std::collections::HashMap;

use async_trait::async_trait;
use pgvector::Vector;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::config::Config;
use crate::model::{Category, FileRecord, IndexedFile, SearchHit, SearchQuery};
use crate::storage::{now_unix, recency_bonus, rrf, Storage};
use crate::{Error, Result};

const OVERFETCH: usize = 5;
const SNIPPET_CHARS: usize = 800;

/// Postgres backend: `pgvector` (HNSW) for KNN, `pg_trgm` for text. The
/// team/large-repo backend; `repo` scopes rows so one instance can hold several.
pub struct PostgresStore {
    pool: PgPool,
    repo: String,
    dims: usize,
    model: String,
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

impl PostgresStore {
    pub async fn open(config: &Config) -> Result<Self> {
        let url = config.storage.postgres_url.as_deref().ok_or_else(|| {
            Error::Config("storage.postgres_url is required for the postgres backend".into())
        })?;
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await
            .map_err(map_err)?;
        Ok(PostgresStore {
            pool,
            repo: config.repo_key(),
            dims: config.embedding.dims,
            model: config.embedding.model.clone(),
        })
    }
}

#[async_trait]
impl Storage for PostgresStore {
    async fn migrate(&self) -> Result<()> {
        let ddl = format!(
            "CREATE EXTENSION IF NOT EXISTS vector;
             CREATE EXTENSION IF NOT EXISTS pg_trgm;
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS files (
                 id               BIGSERIAL PRIMARY KEY,
                 repo             TEXT NOT NULL,
                 path             TEXT NOT NULL,
                 category         TEXT NOT NULL,
                 language         TEXT,
                 size_bytes       BIGINT NOT NULL,
                 sha256           TEXT NOT NULL,
                 mtime            BIGINT NOT NULL,
                 git_last_commit  BIGINT,
                 git_last_author  TEXT,
                 git_commit_count BIGINT,
                 summary          TEXT,
                 UNIQUE (repo, path)
             );
             CREATE TABLE IF NOT EXISTS symbols (
                 id         BIGSERIAL PRIMARY KEY,
                 file_id    BIGINT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 name       TEXT NOT NULL,
                 kind       TEXT NOT NULL,
                 start_line INTEGER NOT NULL,
                 end_line   INTEGER NOT NULL,
                 signature  TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
             CREATE INDEX IF NOT EXISTS idx_symbols_name_trgm ON symbols USING gin (name gin_trgm_ops);
             CREATE TABLE IF NOT EXISTS chunks (
                 id         BIGSERIAL PRIMARY KEY,
                 file_id    BIGINT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 symbol_id  BIGINT REFERENCES symbols(id) ON DELETE SET NULL,
                 content    TEXT NOT NULL,
                 start_line INTEGER NOT NULL,
                 end_line   INTEGER NOT NULL,
                 embedding  VECTOR({dims}) NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_chunks_file ON chunks(file_id);
             CREATE INDEX IF NOT EXISTS idx_chunks_content_trgm ON chunks USING gin (content gin_trgm_ops);
             CREATE INDEX IF NOT EXISTS idx_chunks_embedding ON chunks USING hnsw (embedding vector_cosine_ops);",
            dims = self.dims
        );
        // Only interpolation in `ddl` is the config-controlled integer `dims`.
        sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
            .execute(&self.pool)
            .await
            .map_err(map_err)?;

        let existing_dims: Option<String> =
            sqlx::query_scalar("SELECT value FROM meta WHERE key = 'embedding_dims'")
                .fetch_optional(&self.pool)
                .await
                .map_err(map_err)?;
        let existing_model: Option<String> =
            sqlx::query_scalar("SELECT value FROM meta WHERE key = 'embedding_model'")
                .fetch_optional(&self.pool)
                .await
                .map_err(map_err)?;

        match (existing_dims, existing_model) {
            (Some(d), Some(m)) if d != self.dims.to_string() || m != self.model => {
                Err(Error::Storage(format!(
                    "index was built with model={m} dims={d} but config has model={} dims={}; run `file-sql index --full` to rebuild",
                    self.model, self.dims
                )))
            }
            (Some(_), Some(_)) => Ok(()),
            _ => {
                sqlx::query(
                    "INSERT INTO meta(key, value) VALUES ('embedding_dims', $1), ('embedding_model', $2)
                     ON CONFLICT (key) DO NOTHING",
                )
                .bind(self.dims.to_string())
                .bind(&self.model)
                .execute(&self.pool)
                .await
                .map_err(map_err)?;
                Ok(())
            }
        }
    }

    async fn file_hash(&self, path: &str) -> Result<Option<String>> {
        sqlx::query_scalar("SELECT sha256 FROM files WHERE repo = $1 AND path = $2")
            .bind(&self.repo)
            .bind(path)
            .fetch_optional(&self.pool)
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

        let mut tx = self.pool.begin().await.map_err(map_err)?;

        sqlx::query("DELETE FROM files WHERE repo = $1 AND path = $2")
            .bind(&self.repo)
            .bind(&indexed.file.path)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;

        let f = &indexed.file;
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO files
               (repo, path, category, language, size_bytes, sha256, mtime,
                git_last_commit, git_last_author, git_commit_count, summary)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
             RETURNING id",
        )
        .bind(&self.repo)
        .bind(&f.path)
        .bind(f.category.as_str())
        .bind(&f.language)
        .bind(f.size_bytes as i64)
        .bind(&f.sha256)
        .bind(f.mtime)
        .bind(f.git_last_commit)
        .bind(&f.git_last_author)
        .bind(f.git_commit_count)
        .bind(&f.summary)
        .fetch_one(&mut *tx)
        .await
        .map_err(map_err)?;

        let mut symbol_ids = Vec::with_capacity(indexed.symbols.len());
        for s in &indexed.symbols {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO symbols(file_id, name, kind, start_line, end_line, signature)
                 VALUES ($1,$2,$3,$4,$5,$6) RETURNING id",
            )
            .bind(file_id)
            .bind(&s.name)
            .bind(s.kind.as_str())
            .bind(s.start_line as i32)
            .bind(s.end_line as i32)
            .bind(&s.signature)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_err)?;
            symbol_ids.push(id);
        }

        for c in &indexed.chunks {
            let symbol_id = c.symbol_index.and_then(|i| symbol_ids.get(i).copied());
            sqlx::query(
                "INSERT INTO chunks(file_id, symbol_id, content, start_line, end_line, embedding)
                 VALUES ($1,$2,$3,$4,$5,$6)",
            )
            .bind(file_id)
            .bind(symbol_id)
            .bind(&c.content)
            .bind(c.start_line as i32)
            .bind(c.end_line as i32)
            .bind(Vector::from(c.embedding.clone()))
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        }

        tx.commit().await.map_err(map_err)
    }

    async fn delete_file(&self, path: &str) -> Result<()> {
        sqlx::query("DELETE FROM files WHERE repo = $1 AND path = $2")
            .bind(&self.repo)
            .bind(path)
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn all_paths(&self) -> Result<Vec<String>> {
        sqlx::query_scalar("SELECT path FROM files WHERE repo = $1")
            .bind(&self.repo)
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>> {
        let over = (query.limit.max(1) * OVERFETCH).max(20) as i64;
        let now = now_unix();

        let mut vec_ids: Vec<i64> = Vec::new();
        if !query.embedding.is_empty() {
            vec_ids = sqlx::query_scalar(
                "SELECT c.id FROM chunks c
                 JOIN files f ON f.id = c.file_id
                 WHERE f.repo = $1
                 ORDER BY c.embedding <=> $2
                 LIMIT $3",
            )
            .bind(&self.repo)
            .bind(Vector::from(query.embedding.clone()))
            .bind(over)
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        }

        let mut text_ids: Vec<i64> = Vec::new();
        if query.text.trim().chars().count() >= 3 {
            text_ids = sqlx::query_scalar(
                "SELECT c.id FROM chunks c
                 JOIN files f ON f.id = c.file_id
                 WHERE f.repo = $1 AND c.content ILIKE '%' || $2 || '%'
                 ORDER BY word_similarity($2, c.content) DESC
                 LIMIT $3",
            )
            .bind(&self.repo)
            .bind(&query.text)
            .bind(over)
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        }

        let fused = rrf(&[vec_ids, text_ids]);
        if fused.is_empty() {
            return Ok(Vec::new());
        }
        let candidate_ids: Vec<i64> = fused.keys().copied().collect();

        let rows = sqlx::query(
            "SELECT c.id, c.file_id, c.start_line, c.end_line, s.name,
                    f.path, f.category, f.language, f.summary,
                    f.git_last_commit, f.mtime, c.content
             FROM chunks c
             JOIN files f ON f.id = c.file_id
             LEFT JOIN symbols s ON s.id = c.symbol_id
             WHERE c.id = ANY($1)",
        )
        .bind(&candidate_ids)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let mut best: HashMap<i64, (f64, SearchHit)> = HashMap::new();
        for row in rows {
            let chunk_id: i64 = row.try_get("id").map_err(map_err)?;
            let file_id: i64 = row.try_get("file_id").map_err(map_err)?;
            let path: String = row.try_get("path").map_err(map_err)?;
            let category =
                Category::from_db(&row.try_get::<String, _>("category").map_err(map_err)?);
            let git: Option<i64> = row.try_get("git_last_commit").map_err(map_err)?;
            let mtime: i64 = row.try_get("mtime").map_err(map_err)?;
            let content: String = row.try_get("content").map_err(map_err)?;

            if !query.categories.is_empty() && !query.categories.contains(&category) {
                continue;
            }
            if !query.path_prefixes.is_empty()
                && !query.path_prefixes.iter().any(|p| path.starts_with(p))
            {
                continue;
            }

            let base = fused.get(&chunk_id).copied().unwrap_or(0.0);
            let score = base + recency_bonus(git, mtime, now);
            let entry = best.entry(file_id);
            let start: i32 = row.try_get("start_line").map_err(map_err)?;
            let end: i32 = row.try_get("end_line").map_err(map_err)?;
            let hit = SearchHit {
                path,
                category,
                language: row.try_get("language").map_err(map_err)?,
                summary: row.try_get("summary").map_err(map_err)?,
                score: score as f32,
                start_line: start as u32,
                end_line: end as u32,
                symbol: row.try_get("name").map_err(map_err)?,
                snippet: truncate_snippet(&content),
            };
            entry
                .and_modify(|e| {
                    if score > e.0 {
                        *e = (score, hit.clone());
                    }
                })
                .or_insert((score, hit));
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

    async fn find_symbol(&self, name: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let rows = sqlx::query(
            "SELECT s.name, s.start_line, s.end_line, s.signature,
                    f.path, f.category, f.language, f.summary
             FROM symbols s
             JOIN files f ON f.id = s.file_id
             WHERE f.repo = $1 AND (s.name = $2 OR s.name ILIKE '%' || $2 || '%')
             ORDER BY CASE WHEN s.name = $2 THEN 0 ELSE 1 END, length(s.name)
             LIMIT $3",
        )
        .bind(&self.repo)
        .bind(name)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let mut hits = Vec::with_capacity(rows.len());
        for row in rows {
            let sym_name: String = row.try_get("name").map_err(map_err)?;
            let signature: Option<String> = row.try_get("signature").map_err(map_err)?;
            let category =
                Category::from_db(&row.try_get::<String, _>("category").map_err(map_err)?);
            let start: i32 = row.try_get("start_line").map_err(map_err)?;
            let end: i32 = row.try_get("end_line").map_err(map_err)?;
            let exact = sym_name == name;
            hits.push(SearchHit {
                path: row.try_get("path").map_err(map_err)?,
                category,
                language: row.try_get("language").map_err(map_err)?,
                summary: row.try_get("summary").map_err(map_err)?,
                score: if exact { 1.0 } else { 0.5 },
                start_line: start as u32,
                end_line: end as u32,
                snippet: signature.clone().unwrap_or_else(|| sym_name.clone()),
                symbol: Some(sym_name),
            });
        }
        Ok(hits)
    }

    async fn recently_changed(&self, limit: usize) -> Result<Vec<FileRecord>> {
        let rows = sqlx::query(
            "SELECT path, category, language, size_bytes, sha256, mtime,
                    git_last_commit, git_last_author, git_commit_count, summary
             FROM files
             WHERE repo = $1
             ORDER BY COALESCE(git_last_commit, mtime) DESC
             LIMIT $2",
        )
        .bind(&self.repo)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let category =
                Category::from_db(&row.try_get::<String, _>("category").map_err(map_err)?);
            let size_bytes: i64 = row.try_get("size_bytes").map_err(map_err)?;
            out.push(FileRecord {
                path: row.try_get("path").map_err(map_err)?,
                category,
                language: row.try_get("language").map_err(map_err)?,
                size_bytes: size_bytes as u64,
                sha256: row.try_get("sha256").map_err(map_err)?,
                mtime: row.try_get("mtime").map_err(map_err)?,
                git_last_commit: row.try_get("git_last_commit").map_err(map_err)?,
                git_last_author: row.try_get("git_last_author").map_err(map_err)?,
                git_commit_count: row.try_get("git_commit_count").map_err(map_err)?,
                summary: row.try_get("summary").map_err(map_err)?,
            });
        }
        Ok(out)
    }
}
