#!/usr/bin/env sh
# file-sql installer.
#
#   curl -fsSL https://zottiben.github.io/file-sql/install.sh | sh
#
# Installs the `file-sql` binary (via cargo), writes an AI-free lexical config
# for the current repo, and builds the index.
set -eu

REPO_URL="https://github.com/zottiben/file-sql"
RAW_BASE="https://raw.githubusercontent.com/zottiben/file-sql/main"
say() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33mwarn:\033[0m %s\n' "$1" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# 1. Prerequisites -----------------------------------------------------------
have curl || die "curl is required."
have cargo || die "cargo (Rust toolchain) is required. Install from https://rustup.rs"
have git || warn "git not found - recency ranking will fall back to file mtimes."

# 2. Install the binary ------------------------------------------------------
say "Installing the file-sql binary (this compiles from source and may take a few minutes)..."
cargo install --git "$REPO_URL" file-sql --locked
BIN="$(command -v file-sql || echo "$HOME/.cargo/bin/file-sql")"
say "Installed: $BIN"

# 3. Write per-repo config ---------------------------------------------------
if [ -f ".file-sql/config.toml" ]; then
  say "Keeping existing .file-sql/config.toml"
else
  say "Writing .file-sql/config.toml for this repo (SQLite backend)"
  mkdir -p .file-sql
  cat > .file-sql/config.toml <<EOF
roots = ["."]

[storage]
backend = "sqlite"
sqlite_path = ".file-sql/index.db"

[embedding]
mode = "lexical" # deterministic token hashing; no AI/ML model, no model download
dims = 384
EOF
fi

# 4. Build the initial index -------------------------------------------------
say "Building the initial index..."
"$BIN" index

# 5. Install the agent skill -------------------------------------------------
say "Installing the agent skill into this repo..."
if [ -f install/install-skill.sh ]; then
  sh install/install-skill.sh
else
  curl -fsSL "$RAW_BASE/install/install-skill.sh" | sh
fi

# 6. If this repo already uses Pi, update its project-level MCP config --------
if [ -d .pi ] || [ -f .pi/mcp.json ]; then
  say "Updating existing Pi MCP config..."
  if [ -f install/install-pi-mcp.sh ]; then
    sh install/install-pi-mcp.sh
  else
    curl -fsSL "$RAW_BASE/install/install-pi-mcp.sh" | sh
  fi
fi

# 7. Print MCP wiring --------------------------------------------------------
cat <<EOF

$(printf '\033[1;32mDone.\033[0m') file-sql is installed and this repo is indexed.

Try it:
  file-sql search "where is rate limiting handled"

Wire it into an MCP-capable agent (run the server from this repo directory):

  Claude Code:   claude mcp add file-sql -- file-sql serve
  .mcp.json:     { "mcpServers": { "file-sql": { "command": "file-sql", "args": ["serve"] } } }
  Codex (~/.codex/config.toml):
                 [mcp_servers.file-sql]
                 command = "file-sql"
                 args = ["serve"]

  Pi (.pi/mcp.json):
                 curl -fsSL https://zottiben.github.io/file-sql/install-pi-mcp.sh | sh
                 # writes/merges:
                 { "mcpServers": { "file-sql": { "command": "file-sql", "args": ["serve"], "transport": "stdio", "lifecycle": "eager" } } }

The server reads .file-sql/config.toml from its working directory, so launch it
with the repo as the working directory. Re-run 'file-sql index' (or call the
reindex tool) after changes.
EOF
