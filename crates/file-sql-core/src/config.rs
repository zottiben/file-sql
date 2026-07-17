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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingMode {
    /// Deterministic token-hash vectors. No AI/ML model, no model download, no
    /// network calls; this is the default for company-code-safe indexing.
    #[default]
    Lexical,
    /// Local ONNX embedding model (for semantic search). Still local-only, but
    /// it is an ML model and requires building with the `model-embeddings`
    /// feature.
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Indexing/ranking mode. Default is `lexical` (no AI model).
    #[serde(default)]
    pub mode: EmbeddingMode,
    /// Model identifier used only when `mode = "model"`.
    #[serde(default = "default_model")]
    pub model: String,
    /// Vector dimensionality. For lexical mode this is the token-hash vector
    /// size; for model mode it must match the model.
    #[serde(default = "default_dims")]
    pub dims: usize,
    /// Optional local model directory (`model.onnx` plus tokenizer json files).
    /// When set, the model is loaded from disk instead of downloaded - needed
    /// in air-gapped or TLS-intercepting-proxy environments.
    #[serde(default)]
    pub model_path: Option<PathBuf>,
}

fn default_model() -> String {
    "BAAI/bge-small-en-v1.5".to_string()
}

fn default_dims() -> usize {
    384
}

impl EmbeddingConfig {
    /// Stable key stored in the index metadata so switching between lexical and
    /// model modes refuses to mix incompatible vectors.
    pub fn index_model_key(&self) -> String {
        match self.mode {
            EmbeddingMode::Lexical => "lexical-hash-v1".to_string(),
            EmbeddingMode::Model => format!("model:{}", self.model),
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        EmbeddingConfig {
            mode: EmbeddingMode::Lexical,
            model: default_model(),
            dims: default_dims(),
            model_path: None,
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
    /// Scope key used by the Postgres backend so one instance can hold several
    /// repos. Ignored by SQLite (its file is already per-repo). Defaults to the
    /// canonical path of the first root.
    #[serde(default)]
    pub repo: Option<String>,
}

impl Config {
    /// Stable identifier for this repo within a shared Postgres index.
    pub fn repo_key(&self) -> String {
        if let Some(repo) = &self.repo {
            return repo.clone();
        }
        let root = self
            .roots
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::canonicalize(&root)
            .unwrap_or(root)
            .to_string_lossy()
            .into_owned()
    }
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
