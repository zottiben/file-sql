use std::path::PathBuf;

mod mcp;

use clap::{Parser, Subcommand};
use file_sql_core::config::{Config, EmbeddingMode};
#[cfg(feature = "model-embeddings")]
use file_sql_core::embedding::FastEmbedder;
use file_sql_core::embedding::{Embedder, LexicalEmbedder};
use file_sql_core::indexer::Indexer;
use file_sql_core::model::SearchQuery;
use file_sql_core::storage;

/// file-sql: fast lexical/structural code index for AI agents.
#[derive(Parser)]
#[command(name = "file-sql", version, about)]
struct Cli {
    /// Path to the config file (TOML).
    #[arg(long, global = true, default_value = ".file-sql/config.toml")]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index (or incrementally re-index) the configured roots.
    Index {
        /// Re-embed every file, ignoring content hashes.
        #[arg(long)]
        full: bool,
    },
    /// One-shot lexical/structural search; prints JSON hits.
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Run the MCP server over stdio (via rmcp). A harness launches this
    /// directly; lexical mode is AI-free by default, while optional model mode
    /// keeps the embedding model resident for the session.
    Serve,
    /// Print index stats and configuration.
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Index { full } => {
            let config = load_config(&cli.config)?;
            let store = storage::open(&config).await?;
            let embedder = build_embedder(&config)?;
            let indexer = Indexer::new(&config, store.as_ref(), embedder.as_ref());
            let stats = indexer.run(full).await?;
            println!(
                "indexed {} file(s), skipped {} unchanged, deleted {} stale",
                stats.indexed, stats.skipped, stats.deleted
            );
            Ok(())
        }
        Command::Search { query, limit } => {
            let config = load_config(&cli.config)?;
            let store = storage::open(&config).await?;
            let embedder = build_embedder(&config)?;
            let embedding = embedder.embed_one(&query)?;
            let hits = store
                .search(&SearchQuery {
                    text: query,
                    embedding,
                    limit,
                    categories: vec![],
                    path_prefixes: vec![],
                })
                .await?;
            println!("{}", serde_json::to_string_pretty(&hits)?);
            Ok(())
        }
        Command::Serve => {
            let config = load_config(&cli.config)?;
            mcp::run(config).await
        }
        Command::Status => {
            let config = load_config(&cli.config)?;
            println!("{}", serde_json::to_string_pretty(&config)?);
            Ok(())
        }
    }
}

pub(crate) fn build_embedder(config: &Config) -> anyhow::Result<Box<dyn Embedder>> {
    match config.embedding.mode {
        EmbeddingMode::Lexical => Ok(Box::new(LexicalEmbedder::new(config.embedding.dims)?)),
        EmbeddingMode::Model => build_model_embedder(config),
    }
}

#[cfg(feature = "model-embeddings")]
fn build_model_embedder(config: &Config) -> anyhow::Result<Box<dyn Embedder>> {
    let embedder = match &config.embedding.model_path {
        Some(dir) => FastEmbedder::from_local(dir)?,
        None => FastEmbedder::new(&config.embedding.model, FastEmbedder::default_cache_dir())?,
    };
    if embedder.dims() != config.embedding.dims {
        anyhow::bail!(
            "config embedding.dims={} but model '{}' produces {} dims; fix the config and reindex",
            config.embedding.dims,
            config.embedding.model,
            embedder.dims()
        );
    }
    Ok(Box::new(embedder))
}

#[cfg(not(feature = "model-embeddings"))]
fn build_model_embedder(_config: &Config) -> anyhow::Result<Box<dyn Embedder>> {
    anyhow::bail!(
        "embedding.mode = 'model' requires installing file-sql with `--features model-embeddings`; default installs use AI-free lexical indexing"
    )
}

fn load_config(path: &std::path::Path) -> anyhow::Result<Config> {
    if path.exists() {
        Ok(Config::load(path)?)
    } else {
        anyhow::bail!(
            "no config at {}; run the installer or create it first",
            path.display()
        );
    }
}
