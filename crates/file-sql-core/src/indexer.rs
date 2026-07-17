use std::collections::HashSet;
use std::path::Path;
use std::time::UNIX_EPOCH;

use ignore::WalkBuilder;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::embedding::Embedder;
use crate::model::{Category, Chunk, FileRecord, IndexedFile};
use crate::storage::Storage;
use crate::Result;

/// Lines per chunk and step between chunk starts (the difference is overlap,
/// so matches near a boundary still land inside a chunk).
const CHUNK_WINDOW: usize = 60;
const CHUNK_STRIDE: usize = 45;
const SUMMARY_CHARS: usize = 200;

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct IndexStats {
    pub indexed: usize,
    pub skipped: usize,
    pub deleted: usize,
}

/// Drives one indexing pass: walk the roots, chunk + embed changed files, and
/// reconcile deletions against the store.
pub struct Indexer<'a> {
    config: &'a Config,
    storage: &'a dyn Storage,
    embedder: &'a dyn Embedder,
}

impl<'a> Indexer<'a> {
    pub fn new(config: &'a Config, storage: &'a dyn Storage, embedder: &'a dyn Embedder) -> Self {
        Indexer {
            config,
            storage,
            embedder,
        }
    }

    /// Index every root. With `full`, re-embed unconditionally; otherwise skip
    /// files whose content hash is unchanged.
    pub async fn run(&self, full: bool) -> Result<IndexStats> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stats = IndexStats::default();

        for root in &self.config.roots {
            for result in WalkBuilder::new(root).build() {
                let Ok(entry) = result else { continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let path = entry.path();
                let rel = path.strip_prefix(root).unwrap_or(path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");

                let Ok(bytes) = std::fs::read(path) else {
                    continue;
                };
                if bytes.len() as u64 > self.config.max_file_bytes || is_probably_binary(&bytes) {
                    continue;
                }
                let Ok(content) = String::from_utf8(bytes) else {
                    continue;
                };

                seen.insert(rel_str.clone());
                let hash = sha256_hex(&content);
                if !full && self.storage.file_hash(&rel_str).await? == Some(hash.clone()) {
                    stats.skipped += 1;
                    continue;
                }

                let indexed = self.build_indexed(&rel_str, path, &content, hash)?;
                self.storage.upsert_file(&indexed).await?;
                stats.indexed += 1;
            }
        }

        for indexed_path in self.storage.all_paths().await? {
            if !seen.contains(&indexed_path) {
                self.storage.delete_file(&indexed_path).await?;
                stats.deleted += 1;
            }
        }

        Ok(stats)
    }

    fn build_indexed(
        &self,
        rel: &str,
        abs: &Path,
        content: &str,
        hash: String,
    ) -> Result<IndexedFile> {
        let meta = std::fs::metadata(abs).ok();
        let size_bytes = meta
            .as_ref()
            .map(|m| m.len())
            .unwrap_or(content.len() as u64);
        let mtime = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let raw_chunks = chunk_lines(content);
        let texts: Vec<&str> = raw_chunks.iter().map(|(t, _, _)| t.as_str()).collect();
        let embeddings = self.embedder.embed(&texts)?;
        let chunks = raw_chunks
            .into_iter()
            .zip(embeddings)
            .map(|((text, start_line, end_line), embedding)| Chunk {
                content: text,
                start_line,
                end_line,
                symbol_index: None,
                embedding,
            })
            .collect();

        Ok(IndexedFile {
            file: FileRecord {
                path: rel.to_string(),
                category: classify(rel),
                language: detect_language(rel),
                size_bytes,
                sha256: hash,
                mtime,
                git_last_commit: None,
                git_last_author: None,
                git_commit_count: None,
                summary: make_summary(content),
            },
            symbols: Vec::new(),
            chunks,
        })
    }
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

fn is_probably_binary(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(8192)];
    window.contains(&0)
}

/// Split content into overlapping line windows. Line numbers are 1-based and
/// inclusive; whitespace-only windows are dropped.
fn chunk_lines(content: &str) -> Vec<(String, u32, u32)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let end = (start + CHUNK_WINDOW).min(lines.len());
        let text = lines[start..end].join("\n");
        if !text.trim().is_empty() {
            chunks.push((text, (start + 1) as u32, end as u32));
        }
        if end == lines.len() {
            break;
        }
        start += CHUNK_STRIDE;
    }
    chunks
}

fn make_summary(content: &str) -> Option<String> {
    let mut lines = content.lines().map(str::trim).filter(|l| !l.is_empty());
    let mut summary = lines.next()?.to_string();
    if let Some(second) = lines.next() {
        summary.push(' ');
        summary.push_str(second);
    }
    if summary.chars().count() > SUMMARY_CHARS {
        let truncated: String = summary.chars().take(SUMMARY_CHARS).collect();
        summary = format!("{truncated}…");
    }
    Some(summary)
}

fn detect_language(path: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    let lang = match ext.as_str() {
        "rs" => "rust",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" | "pyi" => "python",
        "go" => "go",
        "java" => "java",
        "rb" => "ruby",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "scala" => "scala",
        "sh" | "bash" | "zsh" => "shell",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "vue" => "vue",
        "svelte" => "svelte",
        "md" | "mdx" => "markdown",
        "toml" | "yaml" | "yml" | "json" => "config",
        _ => return None,
    };
    Some(lang.to_string())
}

/// Best-effort purpose bucket from path + extension. Ordered so the more
/// specific signals (tests, devops) win before broad ones (backend/frontend).
fn classify(path: &str) -> Category {
    let p = path.to_ascii_lowercase();
    let name = p.rsplit('/').next().unwrap_or(&p);
    let ext = Path::new(&p)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let has = |needle: &str| p.contains(needle);

    if has("/test")
        || has("/tests/")
        || has("/__tests__/")
        || has("/spec/")
        || name.starts_with("test_")
        || name.ends_with("_test.go")
        || name.ends_with("_test.py")
        || name.contains(".test.")
        || name.contains(".spec.")
    {
        return Category::Test;
    }

    if name == "dockerfile"
        || name.starts_with("docker-compose")
        || name.ends_with(".tf")
        || has("/.github/")
        || has("/.gitlab")
        || has("/k8s/")
        || has("/kubernetes/")
        || has("/helm/")
        || has("/terraform/")
        || has("/deploy")
        || has("/ci/")
        || has("/.circleci/")
    {
        return Category::Devops;
    }

    match ext {
        "md" | "mdx" | "rst" | "txt" | "adoc" => return Category::Docs,
        "csv" | "tsv" | "parquet" | "sql" => return Category::Data,
        "toml" | "yaml" | "yml" | "json" | "ini" | "cfg" | "conf" | "env" | "properties" => {
            return Category::Config
        }
        "tsx" | "jsx" | "vue" | "svelte" | "css" | "scss" | "sass" | "less" | "html" | "htm" => {
            return Category::Frontend
        }
        "rs" | "go" | "java" | "py" | "rb" | "php" | "kt" | "scala" | "cs" | "c" | "cpp" | "h"
        | "hpp" => return Category::Backend,
        _ => {}
    }

    if has("/frontend/") || has("/web/") || has("/ui/") || has("/components/") || has("/client/") {
        return Category::Frontend;
    }
    if has("/backend/") || has("/server/") || has("/api/") || has("/services/") {
        return Category::Backend;
    }
    if name == "makefile" || name.ends_with(".gradle") || name.ends_with(".bazel") {
        return Category::Build;
    }

    Category::Other
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{Backend, EmbeddingConfig, StorageConfig};
    use crate::embedding::HashEmbedder;
    use crate::model::SearchQuery;
    use crate::storage;

    const DIMS: usize = 32;

    fn config_for(dir: &Path, db: PathBuf) -> Config {
        Config {
            roots: vec![dir.to_path_buf()],
            storage: StorageConfig {
                backend: Backend::Sqlite,
                sqlite_path: db,
                postgres_url: None,
            },
            embedding: EmbeddingConfig {
                model: "hash-test".into(),
                dims: DIMS,
                model_path: None,
            },
            ignore: vec![],
            max_file_bytes: 1 << 20,
            repo: None,
        }
    }

    #[test]
    fn classify_and_language_cover_common_cases() {
        assert_eq!(classify("src/api/user.rs"), Category::Backend);
        assert_eq!(classify("web/components/Button.tsx"), Category::Frontend);
        assert_eq!(classify("crates/core/tests/it.rs"), Category::Test);
        assert_eq!(classify("docker/docker-compose.yml"), Category::Devops);
        assert_eq!(classify("README.md"), Category::Docs);
        assert_eq!(classify("Cargo.toml"), Category::Config);
        assert_eq!(detect_language("a/b.rs").as_deref(), Some("rust"));
        assert_eq!(detect_language("a/b.unknownext"), None);
    }

    #[test]
    fn chunk_lines_windows_with_overlap() {
        let content = (1..=130)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let chunks = chunk_lines(&content);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0].1, 1);
        assert_eq!(chunks[0].2, CHUNK_WINDOW as u32);
        // Second window starts at STRIDE+1, proving the overlap.
        assert_eq!(chunks[1].1, CHUNK_STRIDE as u32 + 1);
        assert_eq!(chunks.last().unwrap().2, 130);
    }

    #[tokio::test]
    async fn indexes_a_tree_then_searches_and_reconciles() {
        let dir = std::env::temp_dir().join(format!("file-sql-idx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("web")).unwrap();
        std::fs::write(
            dir.join("src/auth.rs"),
            "fn login(user: &str, password: &str) -> bool {\n    verify_password(user, password)\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("web/button.tsx"),
            "export function Button() {\n  return <button>click</button>;\n}\n",
        )
        .unwrap();

        let store = storage::open(&config_for(&dir, PathBuf::from(":memory:")))
            .await
            .unwrap();
        let embedder = HashEmbedder::new(DIMS);
        let config = config_for(&dir, PathBuf::from(":memory:"));
        let indexer = Indexer::new(&config, store.as_ref(), &embedder);

        let stats = indexer.run(false).await.unwrap();
        assert_eq!(stats.indexed, 2);
        assert_eq!(stats.skipped, 0);

        // Re-running with no changes skips everything.
        let stats = indexer.run(false).await.unwrap();
        assert_eq!(stats.indexed, 0);
        assert_eq!(stats.skipped, 2);

        // Vector search finds the auth file by its own words.
        let query = SearchQuery {
            text: "login password".into(),
            embedding: embedder.embed_one("login password").unwrap(),
            limit: 5,
            categories: vec![],
            path_prefixes: vec![],
        };
        let hits = store.search(&query).await.unwrap();
        assert_eq!(hits[0].path, "src/auth.rs");
        assert_eq!(hits[0].category, Category::Backend);

        // Deleting a file on disk is reconciled on the next pass.
        std::fs::remove_file(dir.join("web/button.tsx")).unwrap();
        let stats = indexer.run(false).await.unwrap();
        assert_eq!(stats.deleted, 1);
        assert_eq!(
            store.all_paths().await.unwrap(),
            vec!["src/auth.rs".to_string()]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
