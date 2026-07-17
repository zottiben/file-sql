mod postgres;
mod sqlite;

use std::collections::HashMap;

use async_trait::async_trait;

use crate::config::{Backend, Config};
use crate::model::{FileRecord, IndexedFile, SearchHit, SearchQuery};
use crate::Result;

pub use postgres::PostgresStore;
pub use sqlite::SqliteStore;

/// Backend-agnostic index store. Implemented once per engine (SQLite, Postgres).
///
/// The trait is deliberately small: the Rust indexer produces whole
/// [`IndexedFile`]s and the search path issues one hybrid query. Everything
/// about vector types, trigram/FTS indexes, and SQL dialect stays inside the
/// implementation so the rest of the crate never branches on backend.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Create tables, indexes, and load required extensions if missing.
    /// Also pins the embedding model+dims and rejects a changed model.
    async fn migrate(&self) -> Result<()>;

    /// Return the stored content hash for `path`, if the file is indexed.
    /// Used to skip unchanged files during incremental re-indexing.
    async fn file_hash(&self, path: &str) -> Result<Option<String>>;

    /// Insert or replace a file and its symbols/chunks atomically.
    async fn upsert_file(&self, indexed: &IndexedFile) -> Result<()>;

    /// Drop a file and its dependent rows (e.g. after deletion on disk).
    async fn delete_file(&self, path: &str) -> Result<()>;

    /// All indexed file paths, for reconciling against the current tree.
    async fn all_paths(&self) -> Result<Vec<String>>;

    /// Hybrid search: vector similarity fused with trigram text match,
    /// recency-boosted, filtered by category/path prefix.
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>>;

    /// Exact/fuzzy symbol lookup by name.
    async fn find_symbol(&self, name: &str, limit: usize) -> Result<Vec<SearchHit>>;

    /// Files ordered by most recent git/filesystem change.
    async fn recently_changed(&self, limit: usize) -> Result<Vec<FileRecord>>;
}

/// Build and migrate the configured storage backend.
pub async fn open(config: &Config) -> Result<Box<dyn Storage>> {
    let store: Box<dyn Storage> = match config.storage.backend {
        Backend::Sqlite => Box::new(SqliteStore::open(config).await?),
        Backend::Postgres => Box::new(PostgresStore::open(config).await?),
    };
    store.migrate().await?;
    Ok(store)
}

/// Reciprocal-rank-fusion constant. Larger values flatten the contribution of
/// top ranks; 60 is the value from the original RRF paper.
const RRF_K: f64 = 60.0;

/// Fuse several ranked lists of chunk ids into a single score per id.
///
/// Rank fusion sidesteps the fact that cosine distance and trigram/BM25 scores
/// live on incomparable scales: only the *position* in each leg matters.
pub(crate) fn rrf(legs: &[Vec<i64>]) -> HashMap<i64, f64> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for leg in legs {
        for (rank, &id) in leg.iter().enumerate() {
            *scores.entry(id).or_insert(0.0) += 1.0 / (RRF_K + (rank + 1) as f64);
        }
    }
    scores
}

/// Small additive bonus that nudges recently-changed files above ties, decaying
/// with age so it never dominates relevance.
pub(crate) fn recency_bonus(git_last_commit: Option<i64>, mtime: i64, now: i64) -> f64 {
    let ts = git_last_commit.unwrap_or(mtime);
    let age_days = ((now - ts).max(0) as f64) / 86_400.0;
    0.05 / (1.0 + age_days / 30.0)
}

pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::config::{Backend, EmbeddingConfig, StorageConfig};
    use crate::model::{Category, Chunk, FileRecord, IndexedFile, Symbol, SymbolKind};

    const DIMS: usize = 4;

    fn config(path: PathBuf) -> Config {
        Config {
            roots: vec![PathBuf::from(".")],
            storage: StorageConfig {
                backend: Backend::Sqlite,
                sqlite_path: path,
                postgres_url: None,
            },
            embedding: EmbeddingConfig {
                model: "test-model".into(),
                dims: DIMS,
            },
            ignore: vec![],
            max_file_bytes: 1 << 20,
            repo: None,
        }
    }

    fn temp_db() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("file-sql-test-{}-{n}.db", std::process::id()))
    }

    fn indexed(
        path: &str,
        category: Category,
        content: &str,
        symbol: &str,
        embedding: [f32; DIMS],
        git_last_commit: i64,
    ) -> IndexedFile {
        IndexedFile {
            file: FileRecord {
                path: path.into(),
                category,
                language: Some("rust".into()),
                size_bytes: content.len() as u64,
                sha256: format!("sha-{path}"),
                mtime: git_last_commit,
                git_last_commit: Some(git_last_commit),
                git_last_author: Some("tester".into()),
                git_commit_count: Some(1),
                summary: Some(format!("summary of {path}")),
            },
            symbols: vec![Symbol {
                name: symbol.into(),
                kind: SymbolKind::Function,
                start_line: 1,
                end_line: 3,
                signature: Some(format!("fn {symbol}()")),
            }],
            chunks: vec![Chunk {
                content: content.into(),
                start_line: 1,
                end_line: 3,
                symbol_index: Some(0),
                embedding: embedding.to_vec(),
            }],
        }
    }

    fn query(text: &str, embedding: [f32; DIMS], categories: Vec<Category>) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            embedding: embedding.to_vec(),
            limit: 5,
            categories,
            path_prefixes: vec![],
        }
    }

    #[tokio::test]
    async fn sqlite_pipeline_indexes_and_searches() {
        let store = open(&config(PathBuf::from(":memory:"))).await.unwrap();
        store
            .upsert_file(&indexed(
                "src/auth.rs",
                Category::Backend,
                "fn login validates the user password against the store",
                "login",
                [1.0, 0.0, 0.0, 0.0],
                200,
            ))
            .await
            .unwrap();
        store
            .upsert_file(&indexed(
                "web/ui.tsx",
                Category::Frontend,
                "render a clickable button component in the toolbar",
                "Button",
                [0.0, 1.0, 0.0, 0.0],
                100,
            ))
            .await
            .unwrap();

        // Hybrid: vector + text both point at the backend file.
        let hits = store
            .search(&query("login", [1.0, 0.0, 0.0, 0.0], vec![]))
            .await
            .unwrap();
        assert_eq!(hits[0].path, "src/auth.rs");
        assert!(hits[0].snippet.contains("login"));

        // Vector-only query (text too short to trigger the trigram leg).
        let hits = store
            .search(&query("", [0.0, 1.0, 0.0, 0.0], vec![]))
            .await
            .unwrap();
        assert_eq!(hits[0].path, "web/ui.tsx");

        // Category filter excludes the backend file even for its own vector.
        let hits = store
            .search(&query("", [1.0, 0.0, 0.0, 0.0], vec![Category::Frontend]))
            .await
            .unwrap();
        assert!(hits.iter().all(|h| h.category == Category::Frontend));

        // Symbol lookup.
        let syms = store.find_symbol("login", 5).await.unwrap();
        assert_eq!(syms[0].path, "src/auth.rs");
        assert_eq!(syms[0].symbol.as_deref(), Some("login"));

        // Incremental skip signal + recency ordering.
        assert_eq!(
            store.file_hash("src/auth.rs").await.unwrap().as_deref(),
            Some("sha-src/auth.rs")
        );
        let recent = store.recently_changed(10).await.unwrap();
        assert_eq!(recent[0].path, "src/auth.rs");

        // Delete reconciles the tree.
        store.delete_file("src/auth.rs").await.unwrap();
        let paths = store.all_paths().await.unwrap();
        assert_eq!(paths, vec!["web/ui.tsx".to_string()]);
    }

    #[tokio::test]
    async fn sqlite_upsert_replaces_prior_rows() {
        let store = open(&config(PathBuf::from(":memory:"))).await.unwrap();
        let mut file = indexed(
            "src/a.rs",
            Category::Backend,
            "first version content here",
            "alpha",
            [1.0, 0.0, 0.0, 0.0],
            100,
        );
        store.upsert_file(&file).await.unwrap();
        file.file.sha256 = "sha-updated".into();
        file.chunks[0].content = "second version rewritten content".into();
        store.upsert_file(&file).await.unwrap();

        assert_eq!(
            store.file_hash("src/a.rs").await.unwrap().as_deref(),
            Some("sha-updated")
        );
        let hits = store
            .search(&query("rewritten", [1.0, 0.0, 0.0, 0.0], vec![]))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("second version"));
    }

    #[tokio::test]
    async fn sqlite_rejects_changed_embedding_model() {
        let path = temp_db();
        let _ = std::fs::remove_file(&path);
        {
            let store = open(&config(path.clone())).await.unwrap();
            store
                .upsert_file(&indexed(
                    "src/a.rs",
                    Category::Backend,
                    "some content",
                    "a",
                    [1.0, 0.0, 0.0, 0.0],
                    1,
                ))
                .await
                .unwrap();
        }
        let mut changed = config(path.clone());
        changed.embedding.dims = 8;
        let err = open(&changed)
            .await
            .err()
            .expect("expected a model-change error");
        assert!(err.to_string().contains("index --full"), "got: {err}");
        let _ = std::fs::remove_file(&path);
    }

    /// Runs only when `FILE_SQL_TEST_POSTGRES` holds a pgvector connection URL,
    /// so the default `cargo test` stays green without Docker. Bring the backend
    /// up with `docker compose -f docker/docker-compose.postgres.yml up -d`.
    #[tokio::test]
    async fn postgres_pipeline_indexes_and_searches() {
        let Ok(url) = std::env::var("FILE_SQL_TEST_POSTGRES") else {
            return;
        };
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let repo = format!(
            "test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );

        let mut cfg = config(PathBuf::from(":memory:"));
        cfg.storage.backend = Backend::Postgres;
        cfg.storage.postgres_url = Some(url);
        cfg.repo = Some(repo);

        let store = open(&cfg).await.unwrap();
        store
            .upsert_file(&indexed(
                "src/auth.rs",
                Category::Backend,
                "fn login validates the user password against the store",
                "login",
                [1.0, 0.0, 0.0, 0.0],
                200,
            ))
            .await
            .unwrap();
        store
            .upsert_file(&indexed(
                "web/ui.tsx",
                Category::Frontend,
                "render a clickable button component in the toolbar",
                "Button",
                [0.0, 1.0, 0.0, 0.0],
                100,
            ))
            .await
            .unwrap();

        let hits = store
            .search(&query("login", [1.0, 0.0, 0.0, 0.0], vec![]))
            .await
            .unwrap();
        assert_eq!(hits[0].path, "src/auth.rs");

        let hits = store
            .search(&query("", [0.0, 1.0, 0.0, 0.0], vec![]))
            .await
            .unwrap();
        assert_eq!(hits[0].path, "web/ui.tsx");

        let hits = store
            .search(&query("", [1.0, 0.0, 0.0, 0.0], vec![Category::Frontend]))
            .await
            .unwrap();
        assert!(hits.iter().all(|h| h.category == Category::Frontend));

        let syms = store.find_symbol("login", 5).await.unwrap();
        assert_eq!(syms[0].path, "src/auth.rs");

        let recent = store.recently_changed(10).await.unwrap();
        assert_eq!(recent[0].path, "src/auth.rs");

        store.delete_file("src/auth.rs").await.unwrap();
        let paths = store.all_paths().await.unwrap();
        assert_eq!(paths, vec!["web/ui.tsx".to_string()]);

        // Clean up this repo's rows so re-runs stay isolated.
        store.delete_file("web/ui.tsx").await.unwrap();
        assert!(store.all_paths().await.unwrap().is_empty());
    }
}
