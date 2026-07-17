use std::path::PathBuf;

use clap::{Parser, Subcommand};
use file_sql_core::config::Config;
use file_sql_core::embedding::{Embedder, FastEmbedder};
use file_sql_core::indexer::Indexer;
use file_sql_core::model::SearchQuery;
use file_sql_core::storage;

/// file-sql: fast, semantic, structural code index for AI agents.
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
    /// One-shot hybrid search; prints JSON hits.
    Search {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Run the MCP server over stdio (via rmcp). A harness launches this
    /// directly; the embedding model loads once and stays resident for the
    /// session.
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
            let indexer = Indexer::new(&config, store.as_ref(), &embedder);
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
            let _config = load_config(&cli.config)?;
            anyhow::bail!("serve: not implemented yet");
        }
        Command::Status => {
            let config = load_config(&cli.config)?;
            println!("{}", serde_json::to_string_pretty(&config)?);
            Ok(())
        }
    }
}

fn build_embedder(config: &Config) -> anyhow::Result<FastEmbedder> {
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
    Ok(embedder)
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
