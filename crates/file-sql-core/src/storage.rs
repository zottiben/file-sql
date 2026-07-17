use async_trait::async_trait;

use crate::config::Config;
use crate::model::{FileRecord, IndexedFile, SearchHit, SearchQuery};
use crate::{Error, Result};

/// Backend-agnostic index store. Implemented once per engine (SQLite, Postgres).
///
/// The trait is deliberately small: the Rust indexer produces whole
/// [`IndexedFile`]s and the search path issues one hybrid query. Everything
/// about vector types, trigram/FTS indexes, and SQL dialect stays inside the
/// implementation so the rest of the crate never branches on backend.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Create tables, indexes, and load required extensions if missing.
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

    /// Hybrid search: vector similarity fused with trigram/FTS text match,
    /// recency-boosted, filtered by category/path prefix.
    async fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>>;

    /// Exact/fuzzy symbol lookup by name.
    async fn find_symbol(&self, name: &str, limit: usize) -> Result<Vec<SearchHit>>;

    /// Files ordered by most recent git/filesystem change.
    async fn recently_changed(&self, limit: usize) -> Result<Vec<FileRecord>>;
}

/// Build the configured storage backend.
pub async fn open(config: &Config) -> Result<Box<dyn Storage>> {
    match config.storage.backend {
        crate::config::Backend::Sqlite | crate::config::Backend::Postgres => Err(Error::Storage(
            "storage backends are not implemented yet".to_string(),
        )),
    }
}
