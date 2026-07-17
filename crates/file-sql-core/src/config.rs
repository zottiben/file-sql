use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// Which storage engine backs the index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Zero-setup, single-file, per-repo. Best for individual work.
    #[default]
    Sqlite,
    /// Larger or multi-repo indexes on your own Postgres (pgvector). Not yet a
    /// shared multi-user service.
    Postgres,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub backend: Backend,
    /// SQLite file path. Ignored for Postgres.
    #[serde(default = "default_sqlite_path")]
    pub sqlite_path: PathBuf,
    /// Postgres connection string. Ignored for SQLite.
    #[serde(default)]
    pub postgres_url: Option<String>,
}

fn default_sqlite_path() -> PathBuf {
    PathBuf::from(".file-sql/index.db")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Model identifier resolved by the embedding backend (default: bge-small).
    #[serde(default = "default_model")]
    pub model: String,
    /// Vector dimensionality; must match the model above.
    #[serde(default = "default_dims")]
    pub dims: usize,
}

fn default_model() -> String {
    "BAAI/bge-small-en-v1.5".to_string()
}

fn default_dims() -> usize {
    384
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            model: default_model(),
            dims: default_dims(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Repo/dir roots to index. Relative paths resolve against the repo root.
    #[serde(default = "default_roots")]
    pub roots: Vec<PathBuf>,
    pub storage: StorageConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    /// Extra ignore globs applied on top of .gitignore rules.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Skip files larger than this many bytes (default 1 MiB).
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
}

fn default_roots() -> Vec<PathBuf> {
    vec![PathBuf::from(".")]
}

fn default_max_file_bytes() -> u64 {
    1024 * 1024
}

impl Config {
    /// Load config from a TOML file.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        toml::from_str(&raw).map_err(|e| Error::Config(e.to_string()))
    }
}
