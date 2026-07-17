#!/usr/bin/env sh
# Install the file-sql agent skill so an MCP-capable agent knows to prefer the
# file-sql tools over grep.
#
#   Into the current repo (default):
#     curl -fsSL https://zottiben.github.io/file-sql/install-skill.sh | sh
#
#   User-wide (~/.claude/skills):
#     curl -fsSL https://zottiben.github.io/file-sql/install-skill.sh | sh -s -- --user
set -eu

SKILL_NAME="file-sql-search"
RAW_SKILL="https://raw.githubusercontent.com/zottiben/file-sql/main/skill/SKILL.md"

say() { printf '\033[1;34m==>\033[0m %s\n' "$1"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }

scope="project"
for arg in "$@"; do
  case "$arg" in
    --user) scope="user" ;;
    -h|--help) echo "usage: install-skill.sh [--user]"; exit 0 ;;
    *) die "unknown option: $arg (try --user)" ;;
  esac
done

# Source the skill from a local checkout if present, otherwise download it.
src=""
for cand in "skill/SKILL.md" "$(dirname "$0")/../skill/SKILL.md"; do
  if [ -f "$cand" ]; then src="$cand"; break; fi
done
if [ -z "$src" ]; then
  command -v curl >/dev/null 2>&1 || die "curl is required to download the skill."
fi

# Install into every convention so it works across harnesses (Claude Code reads
# .claude/skills; Codex/OpenCode/Pi and others read .agents/skills).
if [ "$scope" = "user" ]; then
  bases="$HOME/.claude/skills"
else
  bases=".claude/skills .agents/skills"
fi

for base in $bases; do
  dest="$base/$SKILL_NAME"
  mkdir -p "$dest"
  if [ -n "$src" ]; then
    cp "$src" "$dest/SKILL.md"
  else
    curl -fsSL "$RAW_SKILL" -o "$dest/SKILL.md"
  fi
  say "Installed skill -> $dest/SKILL.md"
done

say "Done. Restart your agent so it picks up the '$SKILL_NAME' skill."
