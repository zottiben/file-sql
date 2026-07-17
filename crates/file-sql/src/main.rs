use std::path::PathBuf;

use clap::{Parser, Subcommand};
use file_sql_core::config::Config;

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
    /// Run the resident worker: read JSON-RPC requests on stdin, write
    /// responses on stdout. The MCP adapter spawns this as a child process so
    /// the embedding model stays resident for the session - cross-platform,
    /// with no socket files or orphaned daemons.
    Serve,
    /// Print index stats and configuration.
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Index { full } => {
            let _config = load_config(&cli.config)?;
            anyhow::bail!("index: not implemented yet (full={full})");
        }
        Command::Search { query, limit } => {
            let _config = load_config(&cli.config)?;
            anyhow::bail!("search: not implemented yet (query={query:?}, limit={limit})");
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
