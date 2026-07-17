# file-sql

A local code-index MCP server that lets AI agents find the right files fast -
without blindly grepping and re-reading a whole repo.

## Install

```sh
curl -fsSL https://zottiben.github.io/file-sql/install.sh | sh
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

Behind a TLS-intercepting corporate proxy, if `cargo install` fails to clone
over HTTPS, tell cargo to use the git CLI (which trusts the system cert store):

```sh
export CARGO_NET_GIT_FETCH_WITH_CLI=true
```

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

Tools exposed: `search_code`, `find_symbol`, `recently_changed`, `reindex`.

Install the bundled skill so your agent knows to prefer these tools over grep
(the main installer does this automatically; run it standalone with):

```sh
curl -fsSL https://zottiben.github.io/file-sql/install-skill.sh | sh
```

It writes the skill into `.claude/skills/file-sql-search/` and
`.agents/skills/file-sql-search/` in the current repo, so Claude Code, Codex,
OpenCode, and others pick it up. Append `-s -- --user` to install into
`~/.claude/skills/` instead of the current repo.

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

## Storage backends

Pick one at install/config time:

| Backend    | Setup            | Best for                          |
| ---------- | ---------------- | --------------------------------- |
| `sqlite`   | none (one file)  | individual work, per-repo index   |
| `postgres` | Docker container | shared/team, very large indexes   |

SQLite uses `sqlite-vec` + FTS5; Postgres uses `pgvector` + `pg_trgm`. Both sit
behind one `Storage` trait, so the indexer and search code never branch on
backend.

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
