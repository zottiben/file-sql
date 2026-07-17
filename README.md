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

- Git metadata (last-changed, churn) is computed in a single `gix` traversal,
  not one `git log` per file.
- Re-indexing is incremental by content hash; SQLite runs in WAL mode via
  `tokio-rusqlite` so search reads don't block on writes.
- The active embedding model + dimensionality are pinned in a `meta` table;
  changing the model requires a `--full` reindex.
- Postgres builds an HNSW index over the vectors for large indexes; sqlite-vec
  uses brute-force KNN (fast for personal repos, the reason Postgres exists for
  bigger ones). A `repo` scope key lets one Postgres hold multiple repos.
- Unsupported languages fall back to line-window chunking so every text file is
  still searchable; oversized symbols are split to fit the model's context.

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

Early scaffold. Foundation (workspace, config, domain model, storage trait,
Postgres compose) is in place and compiles. Indexer, embeddings, backend
implementations, MCP server, installer, and skill are in progress.

## Development

```sh
cargo check          # build the Rust core + CLI
cargo run -p file-sql -- status
```
