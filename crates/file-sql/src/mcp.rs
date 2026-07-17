use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::transport::stdio;
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};

use file_sql_core::config::Config;
use file_sql_core::embedding::Embedder;
use file_sql_core::indexer::Indexer;
use file_sql_core::model::{Category, SearchQuery};
use file_sql_core::storage::{self, Storage};

/// Shared, immutable server state behind an `Arc` so the rmcp handler can be
/// cloned per request cheaply while the storage connection and configured
/// ranker/embedder are shared.
struct Inner {
    config: Config,
    storage: Box<dyn Storage>,
    embedder: Box<dyn Embedder>,
}

#[derive(Clone)]
pub struct FileSqlServer {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SearchArgs {
    /// Query text for the lexical/structural index. Use likely code terms plus
    /// a short description of the behavior/concept.
    query: String,
    /// Maximum number of files to return. Defaults to 10.
    #[serde(default)]
    limit: Option<usize>,
    /// Restrict to these purpose buckets: backend, frontend, devops, test,
    /// config, docs, data, build, other.
    #[serde(default)]
    categories: Option<Vec<String>>,
    /// Restrict to files whose path starts with this prefix (e.g. "crates/").
    #[serde(default)]
    path_prefix: Option<String>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct FindSymbolArgs {
    /// Symbol name (function, class, struct, etc.) - exact or partial.
    name: String,
    /// Maximum number of matches to return. Defaults to 10.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct RecentArgs {
    /// Maximum number of files to return. Defaults to 20.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct ReindexArgs {
    /// Re-embed every file instead of skipping unchanged ones. Defaults to false.
    #[serde(default)]
    full: Option<bool>,
}

fn to_err(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, ErrorData> {
    let body = serde_json::to_string_pretty(value).map_err(to_err)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(body)]))
}

#[tool_router]
impl FileSqlServer {
    pub fn new(config: Config, storage: Box<dyn Storage>, embedder: Box<dyn Embedder>) -> Self {
        FileSqlServer {
            inner: Arc::new(Inner {
                config,
                storage,
                embedder,
            }),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Lexical + structural code search over the indexed repo (AI-free by default; semantic only if configured with a local model). Returns ranked files with a summary and the best-matching line range - read those lines instead of the whole file. Prefer this over grep for finding where something lives."
    )]
    async fn search_code(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let embedding = self.inner.embedder.embed_one(&args.query).map_err(to_err)?;
        let categories = args
            .categories
            .unwrap_or_default()
            .iter()
            .map(|c| Category::from_db(c))
            .collect();
        let hits = self
            .inner
            .storage
            .search(&SearchQuery {
                text: args.query,
                embedding,
                limit: args.limit.unwrap_or(10),
                categories,
                path_prefixes: args.path_prefix.into_iter().collect(),
            })
            .await
            .map_err(to_err)?;
        json_result(&hits)
    }

    #[tool(
        description = "Find a symbol (function, class, struct, trait, etc.) by name and get its file and line range. Use when you know the identifier and want its definition."
    )]
    async fn find_symbol(
        &self,
        Parameters(args): Parameters<FindSymbolArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let hits = self
            .inner
            .storage
            .find_symbol(&args.name, args.limit.unwrap_or(10))
            .await
            .map_err(to_err)?;
        json_result(&hits)
    }

    #[tool(
        description = "List the most recently changed files (by git/filesystem time). Use to orient on what was touched recently before diving in."
    )]
    async fn recently_changed(
        &self,
        Parameters(args): Parameters<RecentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let files = self
            .inner
            .storage
            .recently_changed(args.limit.unwrap_or(20))
            .await
            .map_err(to_err)?;
        json_result(&files)
    }

    #[tool(
        description = "Re-index the repo so search reflects the latest code. Incremental by default (skips unchanged files); pass full=true to rebuild everything."
    )]
    async fn reindex(
        &self,
        Parameters(args): Parameters<ReindexArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let indexer = Indexer::new(
            &self.inner.config,
            self.inner.storage.as_ref(),
            self.inner.embedder.as_ref(),
        );
        let stats = indexer
            .run(args.full.unwrap_or(false))
            .await
            .map_err(to_err)?;
        json_result(&stats)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FileSqlServer {
    fn get_info(&self) -> ServerInfo {
        let mut server_info = Implementation::from_build_env();
        server_info.name = env!("CARGO_PKG_NAME").into();
        server_info.version = env!("CARGO_PKG_VERSION").into();

        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = server_info;
        info.instructions = Some(
            "Fast lexical + structural code search over this repo (AI-free by default; \
             semantic only if configured with a local model). Use search_code to locate \
             where a behavior or concept lives (returns ranked files + line ranges to read), \
             find_symbol to jump to a definition, recently_changed to see recent edits, and \
             reindex to refresh after changes. Prefer these over grepping and reading whole files."
                .into(),
        );
        info
    }
}

/// Serve the MCP server over stdio until the client disconnects. Storage and
/// the configured ranker/embedder stay resident for the whole session.
pub async fn run(config: Config) -> anyhow::Result<()> {
    let storage = storage::open(&config).await?;
    let embedder = crate::build_embedder(&config)?;
    let service = FileSqlServer::new(config, storage, embedder)
        .serve(stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
