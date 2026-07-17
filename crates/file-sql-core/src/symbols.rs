use tree_sitter::{Language, Node, Parser};

use crate::model::{Symbol, SymbolKind};

const SIGNATURE_CHARS: usize = 160;

type KindMap = &'static [(&'static str, SymbolKind)];

const RUST: KindMap = &[
    ("function_item", SymbolKind::Function),
    ("struct_item", SymbolKind::Struct),
    ("union_item", SymbolKind::Struct),
    ("enum_item", SymbolKind::Enum),
    ("trait_item", SymbolKind::Trait),
    ("type_item", SymbolKind::Type),
    ("const_item", SymbolKind::Constant),
    ("static_item", SymbolKind::Constant),
    ("mod_item", SymbolKind::Module),
    ("macro_definition", SymbolKind::Other),
];

const PYTHON: KindMap = &[
    ("function_definition", SymbolKind::Function),
    ("class_definition", SymbolKind::Class),
];

const JAVASCRIPT: KindMap = &[
    ("function_declaration", SymbolKind::Function),
    ("generator_function_declaration", SymbolKind::Function),
    ("class_declaration", SymbolKind::Class),
    ("method_definition", SymbolKind::Method),
];

const TYPESCRIPT: KindMap = &[
    ("function_declaration", SymbolKind::Function),
    ("generator_function_declaration", SymbolKind::Function),
    ("class_declaration", SymbolKind::Class),
    ("abstract_class_declaration", SymbolKind::Class),
    ("method_definition", SymbolKind::Method),
    ("interface_declaration", SymbolKind::Interface),
    ("type_alias_declaration", SymbolKind::Type),
    ("enum_declaration", SymbolKind::Enum),
];

const GO: KindMap = &[
    ("function_declaration", SymbolKind::Function),
    ("method_declaration", SymbolKind::Method),
    ("type_spec", SymbolKind::Type),
];

fn language_spec(language: &str) -> Option<(Language, KindMap)> {
    let spec = match language {
        "rust" => (Language::new(tree_sitter_rust::LANGUAGE), RUST),
        "python" => (Language::new(tree_sitter_python::LANGUAGE), PYTHON),
        "javascript" => (Language::new(tree_sitter_javascript::LANGUAGE), JAVASCRIPT),
        "typescript" => (
            Language::new(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
            TYPESCRIPT,
        ),
        "go" => (Language::new(tree_sitter_go::LANGUAGE), GO),
        _ => return None,
    };
    Some(spec)
}

/// Whether a language string is supported for structural symbol extraction.
pub fn is_supported(language: &str) -> bool {
    language_spec(language).is_some()
}

/// Extract named definitions (functions, classes, structs, ...) from `source`
/// using tree-sitter. Returns an empty vec for unsupported languages or parse
/// failures - the caller still has line-window chunks for search.
pub fn extract_symbols(language: &str, source: &str) -> Vec<Symbol> {
    let Some((lang, kinds)) = language_spec(language) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&lang).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };

    let bytes = source.as_bytes();
    let mut symbols = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if let Some(kind) = kind_for(kinds, node.kind()) {
            if let Some(name) = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
            {
                symbols.push(Symbol {
                    name: name.to_string(),
                    kind,
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    signature: signature(&node, source),
                });
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            stack.push(child);
        }
    }
    symbols
}

fn kind_for(kinds: KindMap, node_kind: &str) -> Option<SymbolKind> {
    kinds
        .iter()
        .find(|(k, _)| *k == node_kind)
        .map(|(_, kind)| *kind)
}

fn signature(node: &Node, source: &str) -> Option<String> {
    let first = source.get(node.byte_range())?.lines().next()?.trim();
    if first.is_empty() {
        return None;
    }
    if first.chars().count() > SIGNATURE_CHARS {
        Some(format!(
            "{}…",
            first.chars().take(SIGNATURE_CHARS).collect::<String>()
        ))
    } else {
        Some(first.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_symbols() {
        let src = "pub fn login(user: &str) -> bool { true }\nstruct Session { id: u64 }\ntrait Auth {}\n";
        let syms = extract_symbols("rust", src);
        let names: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert!(names.contains(&("login", SymbolKind::Function)));
        assert!(names.contains(&("Session", SymbolKind::Struct)));
        assert!(names.contains(&("Auth", SymbolKind::Trait)));
        let login = syms.iter().find(|s| s.name == "login").unwrap();
        assert_eq!(login.start_line, 1);
        assert!(login.signature.as_deref().unwrap().contains("fn login"));
    }

    #[test]
    fn extracts_python_and_methods() {
        let src = "class Server:\n    def handle(self):\n        pass\n\ndef main():\n    pass\n";
        let syms = extract_symbols("python", src);
        let names: Vec<_> = syms.iter().map(|s| (s.name.as_str(), s.kind)).collect();
        assert!(names.contains(&("Server", SymbolKind::Class)));
        assert!(names.contains(&("handle", SymbolKind::Function)));
        assert!(names.contains(&("main", SymbolKind::Function)));
    }

    #[test]
    fn unsupported_language_is_empty() {
        assert!(extract_symbols("cobol", "IDENTIFICATION DIVISION.").is_empty());
    }
}
