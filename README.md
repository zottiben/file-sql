# file-sql

A local code-index MCP server that lets AI agents find the right files fast -
without blindly grepping and re-reading a whole repo.

## Why

When an agent needs to locate where a change goes or hunt a bug, it tends to
grep for guessed keywords across the entire tree and then read whole files.
That is slow and burns tokens. `file-sql` replaces that loop with a purpose-built
index that answers three things grep cannot:

- **Semantic discovery** - "where is rate limiting handled" finds the right
  files even when they never contain the words "rate limit" (vector search).
- **Token reduction** - results come back as ranked files with a short summary
  and the exact matching line range, so the model reads 20 lines, not 500.
- **Structural precision + recency** - jump straight to a symbol's definition,
  and recently-changed files are boosted so the file you just touched surfaces
  first.

Exact string/regex match stays available as a fallback for when the model
already knows the literal it wants.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/zottiben/file-sql/main/install/install.sh | sh
```

Run it from the repo you want to index. It:

1. installs the `file-sql` binary with `cargo install` (needs a Rust toolchain - https://rustup.rs),
2. pre-downloads the local embedding model with `curl` (works behind corporate
   TLS-intercepting proxies, where the built-in downloader's bundled roots don't
   trust the proxy CA),
3. writes `.file-sql/config.toml` for the repo and builds the initial index.

Prerequisites: `curl`, `cargo`, and (optionally) `git` for recency ranking. The
default SQLite backend needs nothing else; the Postgres backend also needs
Docker (see [Storage backends](#storage-backends)).

## Usage

```sh
file-sql index          # index the configured roots (incremental; --full rebuilds)
file-sql search "how are embeddings generated locally"   # ranked JSON hits
file-sql serve          # run the MCP server over stdio
file-sql status         # print the resolved config
```

## Use it from an AI agent (MCP)

`file-sql serve` is a stdio MCP server, so any MCP-capable harness can call it.
Launch it with the target repo as the working directory (it reads
`.file-sql/config.toml` from there).

- Claude Code: `claude mcp add file-sql -- file-sql serve`
- `.mcp.json`:
  ```json
  { "mcpServers": { "file-sql": { "command": "file-sql", "args": ["serve"] } } }
  ```
- Codex (`~/.codex/config.toml`):
  ```toml
  [mcp_servers.file-sql]
  command = "file-sql"
  args = ["serve"]
  ```

Tools exposed: `search_code`, `find_symbol`, `recently_changed`, `reindex`. Add
`skill/SKILL.md` to your agent so it knows to prefer these over grep.

## Configuration

`.file-sql/config.toml`:

```toml
roots = ["."]               # directories to index
max_file_bytes = 1048576    # skip files larger than this

[storage]
backend = "sqlite"                  # or "postgres"
sqlite_path = ".file-sql/index.db"
# postgres_url = "postgres://file_sql:file_sql@localhost:5433/file_sql"
# repo = "my-repo"                  # scope key when several repos share one Postgres

[embedding]
model = "bge-small-en-v1.5"         # bge-small | all-minilm-l6 | bge-base | bge-large
dims = 384                          # must match the model
# model_path = "/path/to/model-dir" # load a pre-downloaded model (offline / behind a proxy)
```

## Architecture

```
crates/
  file-sql-core/   Rust: config, storage trait, indexer, search, embeddings
  file-sql/        Rust bin: `index | search | serve | status`
docker/            docker-compose for the Postgres + pgvector backend
skill/             bundled agent skill (when/how to use the tools)
install/           curl installer
```

`file-sql` is a single Rust binary. The `serve` subcommand runs the MCP server
directly via `rmcp` (the Rust MCP SDK) over stdio, so a harness launches
`file-sql serve` as its MCP server - no second runtime, no IPC hop, no socket
files. The embedding model loads once and stays resident for the session, and
because it speaks MCP over stdio it plugs into any MCP-capable harness (Claude
Code, Codex, OpenCode, Pi, ...). The same binary also exposes one-shot
`index` / `search` / `status` for scripting and debugging.

### Scaling notes

- Git metadata (last-changed, author, churn) is computed in a single `git log`
  traversal per index, not one call per file.
- Re-indexing is incremental by content hash; SQLite runs in WAL mode via
  `tokio-rusqlite` so search reads don't block on writes.
- The active embedding model + dimensionality are pinned in a `meta` table;
  changing the model requires a `--full` reindex.
- Both backends rank by cosine similarity: Postgres builds an HNSW index over
  the vectors for large indexes; sqlite-vec uses brute-force KNN (fast for
  personal repos, the reason Postgres exists for bigger ones). A `repo` scope
  key lets one Postgres hold multiple repos.
- Files are chunked into overlapping line windows, so every text file is
  searchable regardless of language; for Rust, Python, JavaScript, TypeScript,
  and Go, tree-sitter also extracts named definitions for `find_symbol`. Search
  fuses the vector and trigram legs with reciprocal-rank fusion and a recency
  boost.

## Storage backends

Pick one at install/config time:

| Backend    | Setup            | Best for                          |
| ---------- | ---------------- | --------------------------------- |
| `sqlite`   | none (one file)  | individual work, per-repo index   |
| `postgres` | Docker container | shared/team, very large indexes   |

SQLite uses `sqlite-vec` + FTS5; Postgres uses `pgvector` + `pg_trgm`. Both sit
behind one `Storage` trait, so the indexer and search code never branch on
backend.

## Status

Works end to end: `index`, `search`, `find_symbol`, and the `serve` MCP server
run against a real local embedding model over both SQLite and Postgres. Storage,
indexer, embeddings, git metadata, tree-sitter symbol extraction (Rust, Python,
JavaScript, TypeScript, Go), the `rmcp` server, the installer, and the skill are
all in place and tested (SQLite + the pipeline in CI; Postgres validated against
the pgvector container).

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                       # SQLite + pipeline; Postgres test is env-gated
cargo run -p file-sql -- status
```

To run the Postgres-backed test, start the container and point the test at it:

```sh
docker compose -f docker/docker-compose.postgres.yml up -d
FILE_SQL_TEST_POSTGRES=postgres://file_sql:file_sql@localhost:5433/file_sql cargo test
```
