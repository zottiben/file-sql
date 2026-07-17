#!/usr/bin/env sh
# Install/update the Pi MCP config for file-sql in the current repo.
#
#   curl -fsSL https://zottiben.github.io/file-sql/install-pi-mcp.sh | sh
#
# Writes/merges .pi/mcp.json using Pi's MCP shape:
#   transport = "stdio", lifecycle = "eager"
set -eu

CONFIG="${PI_MCP_CONFIG:-.pi/mcp.json}"

say() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
warn() { printf '\033[1;33mwarn:\033[0m %s\n' "$1" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

print_snippet() {
  cat <<'EOF'
{
  "mcpServers": {
    "file-sql": {
      "command": "file-sql",
      "args": ["serve"],
      "transport": "stdio",
      "lifecycle": "eager"
    }
  }
}
EOF
}

write_new() {
  mkdir -p "$(dirname "$CONFIG")"
  print_snippet > "$CONFIG"
  say "Wrote Pi MCP config -> $CONFIG"
}

if [ ! -f "$CONFIG" ]; then
  write_new
  exit 0
fi

if ! have python3; then
  warn "python3 not found; cannot safely merge existing $CONFIG. Add this under mcpServers manually:"
  cat <<'EOF'
"file-sql": {
  "command": "file-sql",
  "args": ["serve"],
  "transport": "stdio",
  "lifecycle": "eager"
}
EOF
  exit 0
fi

PI_MCP_CONFIG="$CONFIG" python3 <<'PY'
import json
import os
from pathlib import Path

path = Path(os.environ["PI_MCP_CONFIG"])
with path.open("r", encoding="utf-8") as f:
    data = json.load(f)

if not isinstance(data, dict):
    raise SystemExit(f"error: {path} must contain a JSON object")

servers = data.setdefault("mcpServers", {})
if not isinstance(servers, dict):
    raise SystemExit(f"error: {path}.mcpServers must be a JSON object")

servers["file-sql"] = {
    "command": "file-sql",
    "args": ["serve"],
    "transport": "stdio",
    "lifecycle": "eager",
}

path.parent.mkdir(parents=True, exist_ok=True)
with path.open("w", encoding="utf-8") as f:
    json.dump(data, f, indent=2)
    f.write("\n")
PY

say "Updated Pi MCP config -> $CONFIG"
