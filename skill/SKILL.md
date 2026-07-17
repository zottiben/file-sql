---
name: file-sql-search
description: Find code by meaning and structure using the file-sql MCP instead of grepping and reading whole files. Use when locating where a feature or bug lives, jumping to a symbol's definition, or orienting on what changed recently in the repo.
---

# file-sql: search the codebase efficiently

This repo is indexed by **file-sql**, an MCP server that does semantic +
structural code search. Prefer its tools over blind `grep`/`rg` and over reading
whole files: they return ranked files with a short summary and the exact
matching line range, so you read ~20 lines instead of 500.

## When to use which tool

- **`search_code`** - your default for "where is X handled?" Pass a natural
  description of the behavior/concept, not just keywords (it's semantic, so it
  finds the right files even when they don't contain your words). Optional
  `categories` (backend, frontend, devops, test, config, docs, data, build) and
  `path_prefix` narrow the search. Read the returned `path` at the given
  `start_line`-`end_line` before opening the whole file.
- **`find_symbol`** - when you already know an identifier (function, class,
  struct, trait) and want its definition and location.
- **`recently_changed`** - to orient on what was touched recently (git/mtime)
  before diving in, e.g. when investigating a regression.
- **`reindex`** - after you've edited files, so search reflects the new state.
  Incremental by default; pass `full=true` only to rebuild everything.

## Workflow

1. Start with `search_code` using a description of what you're looking for.
2. Read only the returned line ranges of the top hits.
3. Use `find_symbol` to jump to specific definitions you spotted.
4. After making changes, call `reindex` so later searches stay accurate.

## When NOT to use it

- Exact string/regex sweeps across the tree (e.g. finding every call site of a
  literal) are still a job for `rg`.
- If the index seems stale (search misses a file you know exists), call
  `reindex` first.
