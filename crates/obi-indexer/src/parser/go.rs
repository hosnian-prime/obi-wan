use std::path::Path;

use obi_core::node::{Language, NodeType};
use tree_sitter::Parser;

use super::{CallSite, LanguageParser, RawNode};

pub struct GoParser;

impl LanguageParser for GoParser {
    fn language(&self) -> Language {
        Language::Go
    }

    fn file_extensions(&self) -> &[&str] {
        &["go"]
    }

    fn extract_nodes(&self, source: &str, _path: &Path) -> Vec<RawNode> {
        let (nodes, _tree) = self.parse_with_tree(source, None);
        nodes
    }

    fn extract_nodes_incremental(
        &self,
        source: &str,
        _path: &Path,
        old_tree: Option<&tree_sitter::Tree>,
    ) -> (Vec<RawNode>, Option<tree_sitter::Tree>) {
        self.parse_with_tree(source, old_tree)
    }

    fn resolve_imports(&self, source: &str) -> Vec<String> {
        let mut parser = Parser::new();
        let lang = tree_sitter_go::LANGUAGE;
        parser
            .set_language(&lang.into())
            .expect("failed to set Go grammar");

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut imports = Vec::new();
        Self::walk_imports(tree.root_node(), source, &mut imports);
        imports
    }

    fn resolve_calls(&self, source: &str) -> Vec<CallSite> {
        let mut parser = Parser::new();
        let lang = tree_sitter_go::LANGUAGE;
        parser
            .set_language(&lang.into())
            .expect("failed to set Go grammar");

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut calls = Vec::new();
        Self::walk_calls(tree.root_node(), source, &mut calls);
        calls
    }
}

impl GoParser {
    fn parse_with_tree(
        &self,
        source: &str,
        old_tree: Option<&tree_sitter::Tree>,
    ) -> (Vec<RawNode>, Option<tree_sitter::Tree>) {
        let mut parser = Parser::new();
        let lang = tree_sitter_go::LANGUAGE;
        parser
            .set_language(&lang.into())
            .expect("failed to set Go grammar");

        let tree = match parser.parse(source, old_tree) {
            Some(t) => t,
            None => return (Vec::new(), None),
        };

        let mut nodes = Vec::new();
        Self::walk(tree.root_node(), source, &mut nodes);
        (nodes, Some(tree))
    }

    fn walk(node: tree_sitter::Node, source: &str, out: &mut Vec<RawNode>) {
        match node.kind() {
            "function_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(RawNode {
                        name: name.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        node_type: NodeType::Function,
                        content: node.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        line_start: node.start_position().row,
                        line_end: node.end_position().row,
                    });
                }
            }
            "method_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(RawNode {
                        name: name.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        node_type: NodeType::Method,
                        content: node.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        line_start: node.start_position().row,
                        line_end: node.end_position().row,
                    });
                }
            }
            "type_declaration" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "type_spec" {
                        if let Some(name) = child.child_by_field_name("name") {
                            let type_node = child.child_by_field_name("type");
                            let ntype = match type_node.map(|t| t.kind()) {
                                Some("struct_type") => NodeType::Struct,
                                Some("interface_type") => NodeType::Trait,
                                _ => NodeType::Struct,
                            };
                            out.push(RawNode {
                                name: name
                                    .utf8_text(source.as_bytes())
                                    .unwrap_or("")
                                    .to_string(),
                                node_type: ntype,
                                content: child
                                    .utf8_text(source.as_bytes())
                                    .unwrap_or("")
                                    .to_string(),
                                line_start: child.start_position().row,
                                line_end: child.end_position().row,
                            });
                        }
                    }
                }
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    Self::walk(child, source, out);
                }
            }
        }
    }

    /// Extract imported package names.
    /// `import "fmt"` → "fmt"
    /// `import f "fmt"` → "f"
    fn walk_imports(node: tree_sitter::Node, source: &str, out: &mut Vec<String>) {
        match node.kind() {
            "import_spec" => {
                // Check for alias first
                if let Some(name) = node.child_by_field_name("name") {
                    let alias = name.utf8_text(source.as_bytes()).unwrap_or("");
                    if !alias.is_empty() && alias != "." && alias != "_" {
                        out.push(alias.to_string());
                        return;
                    }
                }
                // No alias — use last segment of path
                if let Some(path) = node.child_by_field_name("path") {
                    let text = path.utf8_text(source.as_bytes()).unwrap_or("");
                    let clean = text.trim_matches('"');
                    let name = clean.rsplit('/').next().unwrap_or(clean);
                    if !name.is_empty() {
                        out.push(name.to_string());
                    }
                }
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    Self::walk_imports(child, source, out);
                }
            }
        }
    }

    fn walk_calls(node: tree_sitter::Node, source: &str, out: &mut Vec<CallSite>) {
        if node.kind() == "call_expression" {
            if let Some(func) = node.child_by_field_name("function") {
                let name = func.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                let short = name.rsplit('.').next().unwrap_or(&name).to_string();
                out.push(CallSite {
                    caller_line: node.start_position().row,
                    callee_name: short,
                });
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk_calls(child, source, out);
        }
    }
}
