pub mod generic;
pub mod go;
pub mod python;
pub mod rust;
pub mod typescript;

use std::path::Path;

use obi_core::node::Language;

/// Raw extracted node from tree-sitter, before becoming a SemanticNode.
#[derive(Debug, Clone)]
pub struct RawNode {
    pub name: String,
    pub node_type: obi_core::node::NodeType,
    pub content: String,
    pub line_start: usize,
    pub line_end: usize,
}

/// A call site found in source code.
#[derive(Debug, Clone)]
pub struct CallSite {
    pub caller_line: usize,
    pub callee_name: String,
}

/// Language-agnostic parser trait. Each language implements this.
pub trait LanguageParser: Send + Sync {
    fn language(&self) -> Language;
    fn extract_nodes(&self, source: &str, path: &Path) -> Vec<RawNode>;
    fn resolve_calls(&self, source: &str) -> Vec<CallSite>;
    fn file_extensions(&self) -> &[&str];

    /// Extract imported symbol names from use/import statements.
    fn resolve_imports(&self, _source: &str) -> Vec<String> {
        Vec::new()
    }

    /// Incremental parsing: pass old tree for faster re-parse.
    /// Returns (nodes, new_tree_for_caching).
    /// Default impl ignores old tree and delegates to extract_nodes.
    fn extract_nodes_incremental(
        &self,
        source: &str,
        path: &Path,
        _old_tree: Option<&tree_sitter::Tree>,
    ) -> (Vec<RawNode>, Option<tree_sitter::Tree>) {
        (self.extract_nodes(source, path), None)
    }
}

/// Select the right parser for a file path.
pub fn parser_for_path(path: &Path) -> Box<dyn LanguageParser> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match ext {
        "rs" => Box::new(rust::RustParser),
        "py" => Box::new(python::PythonParser),
        "ts" | "tsx" => Box::new(typescript::TypeScriptParser),
        "js" | "jsx" => Box::new(typescript::TypeScriptParser),
        "go" => Box::new(go::GoParser),
        _ => Box::new(generic::GenericParser),
    }
}
