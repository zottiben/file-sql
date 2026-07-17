#!/usr/bin/env sh
# file-sql installer.
#
#   curl -fsSL https://raw.githubusercontent.com/zottiben/file-sql/main/install/install.sh | sh
#
# Installs the `file-sql` binary (via cargo), pre-downloads the local embedding
# model with curl (works behind TLS-intercepting proxies where the built-in
# downloader can't), writes a config for the current repo, and builds the index.
set -eu

REPO_URL="https://github.com/zottiben/file-sql"
RAW_BASE="https://raw.githubusercontent.com/zottiben/file-sql/main"
MODEL_REPO="https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main"
CACHE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/file-sql"
MODEL_DIR="$CACHE_DIR/models/bge-small"

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

# 3. Pre-download the embedding model ---------------------------------------
if [ -f "$MODEL_DIR/model.onnx" ]; then
  say "Embedding model already present at $MODEL_DIR"
else
  say "Downloading the bge-small embedding model (~130 MB) into $MODEL_DIR ..."
  mkdir -p "$MODEL_DIR"
  curl -fsSL -o "$MODEL_DIR/model.onnx"              "$MODEL_REPO/onnx/model.onnx"
  curl -fsSL -o "$MODEL_DIR/tokenizer.json"          "$MODEL_REPO/tokenizer.json"
  curl -fsSL -o "$MODEL_DIR/config.json"             "$MODEL_REPO/config.json"
  curl -fsSL -o "$MODEL_DIR/special_tokens_map.json" "$MODEL_REPO/special_tokens_map.json"
  curl -fsSL -o "$MODEL_DIR/tokenizer_config.json"   "$MODEL_REPO/tokenizer_config.json"
fi

# 4. Write per-repo config ---------------------------------------------------
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
model = "bge-small-en-v1.5"
dims = 384
model_path = "$MODEL_DIR"
EOF
fi

# 5. Build the initial index -------------------------------------------------
say "Building the initial index..."
"$BIN" index

# 6. Install the agent skill -------------------------------------------------
say "Installing the agent skill into this repo..."
if [ -f install/install-skill.sh ]; then
  sh install/install-skill.sh
else
  curl -fsSL "$RAW_BASE/install/install-skill.sh" | sh
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

The server reads .file-sql/config.toml from its working directory, so launch it
with the repo as the working directory. Re-run 'file-sql index' (or call the
reindex tool) after changes.
EOF
