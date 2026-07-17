use serde::{Deserialize, Serialize};

/// Coarse purpose bucket used to narrow searches (e.g. "only backend files").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Backend,
    Frontend,
    Devops,
    Test,
    Config,
    Docs,
    Data,
    Build,
    Other,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Backend => "backend",
            Category::Frontend => "frontend",
            Category::Devops => "devops",
            Category::Test => "test",
            Category::Config => "config",
            Category::Docs => "docs",
            Category::Data => "data",
            Category::Build => "build",
            Category::Other => "other",
        }
    }

    /// Parse a category from its stored string, defaulting to `Other`.
    pub fn from_db(s: &str) -> Category {
        match s {
            "backend" => Category::Backend,
            "frontend" => Category::Frontend,
            "devops" => Category::Devops,
            "test" => Category::Test,
            "config" => Category::Config,
            "docs" => Category::Docs,
            "data" => Category::Data,
            "build" => Category::Build,
            _ => Category::Other,
        }
    }
}

/// A single tracked file and its derived metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub path: String,
    pub category: Category,
    pub language: Option<String>,
    pub size_bytes: u64,
    /// Content hash; used to skip re-indexing unchanged files.
    pub sha256: String,
    /// Filesystem mtime as unix seconds.
    pub mtime: i64,
    /// Unix seconds of the last git commit touching this file, if tracked.
    pub git_last_commit: Option<i64>,
    pub git_last_author: Option<String>,
    /// Number of commits touching this file (churn signal for ranking).
    pub git_commit_count: Option<i64>,
    /// Cheap structural summary (leading doc comment + symbol outline).
    pub summary: Option<String>,
}

/// A named code entity extracted via tree-sitter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
    Type,
    Constant,
    Module,
    Other,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Interface => "interface",
            SymbolKind::Trait => "trait",
            SymbolKind::Type => "type",
            SymbolKind::Constant => "constant",
            SymbolKind::Module => "module",
            SymbolKind::Other => "other",
        }
    }

    /// Parse a symbol kind from its stored string, defaulting to `Other`.
    pub fn from_db(s: &str) -> SymbolKind {
        match s {
            "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "class" => SymbolKind::Class,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "interface" => SymbolKind::Interface,
            "trait" => SymbolKind::Trait,
            "type" => SymbolKind::Type,
            "constant" => SymbolKind::Constant,
            "module" => SymbolKind::Module,
            _ => SymbolKind::Other,
        }
    }
}

/// An embeddable slice of a file (usually one symbol body).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub content: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Index of the owning symbol within the file's symbol list, if any.
    pub symbol_index: Option<usize>,
    /// Dense embedding of `content`; length must match the active model.
    pub embedding: Vec<f32>,
}

/// Everything the indexer produces for one file in a single pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    pub file: FileRecord,
    pub symbols: Vec<Symbol>,
    pub chunks: Vec<Chunk>,
}

/// A search request against the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    /// Query embedding for the vector leg of the search.
    pub embedding: Vec<f32>,
    pub limit: usize,
    pub categories: Vec<Category>,
    /// Restrict to paths under any of these prefixes when non-empty.
    pub path_prefixes: Vec<String>,
}

/// One ranked result: a file plus its best-matching span.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub category: Category,
    pub language: Option<String>,
    pub summary: Option<String>,
    pub score: f32,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol: Option<String>,
    pub snippet: String,
}
