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
mcp/               Bun/TS MCP server (thin stdio adapter over the daemon)
docker/            docker-compose for the Postgres + pgvector backend
skill/             bundled agent skill (when/how to use the tools)
install/           curl installer
```

The **Rust core** owns the heavy path (walking, tree-sitter chunking, local
embeddings, storage, hybrid search) and runs as a one-shot CLI or as a resident
daemon that keeps the embedding model in memory for sub-second repeated queries.
The **Bun/TS MCP server** is a thin adapter that exposes MCP tools over stdio and
talks to the daemon, so it works with any MCP-capable harness (Claude Code,
Codex, OpenCode, Pi, ...).

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
implementations, daemon, MCP server, installer, and skill are in progress.

## Development

```sh
cargo check          # build the Rust core + CLI
cargo run -p file-sql -- status
```
