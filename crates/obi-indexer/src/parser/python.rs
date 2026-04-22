use std::path::Path;

use obi_core::node::{Language, NodeType};
use tree_sitter::Parser;

use super::{CallSite, LanguageParser, RawNode};

pub struct PythonParser;

impl LanguageParser for PythonParser {
    fn language(&self) -> Language {
        Language::Python
    }

    fn file_extensions(&self) -> &[&str] {
        &["py"]
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
        let lang = tree_sitter_python::LANGUAGE;
        parser
            .set_language(&lang.into())
            .expect("failed to set Python grammar");

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
        let lang = tree_sitter_python::LANGUAGE;
        parser
            .set_language(&lang.into())
            .expect("failed to set Python grammar");

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut calls = Vec::new();
        Self::walk_calls(tree.root_node(), source, &mut calls);
        calls
    }
}

impl PythonParser {
    fn parse_with_tree(
        &self,
        source: &str,
        old_tree: Option<&tree_sitter::Tree>,
    ) -> (Vec<RawNode>, Option<tree_sitter::Tree>) {
        let mut parser = Parser::new();
        let lang = tree_sitter_python::LANGUAGE;
        parser
            .set_language(&lang.into())
            .expect("failed to set Python grammar");

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
            "function_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node
                        .utf8_text(source.as_bytes())
                        .unwrap_or("")
                        .to_string();
                    out.push(RawNode {
                        name,
                        node_type: NodeType::Function,
                        content: node.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        line_start: node.start_position().row,
                        line_end: node.end_position().row,
                    });
                }
            }
            "class_definition" => {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let name = name_node
                        .utf8_text(source.as_bytes())
                        .unwrap_or("")
                        .to_string();
                    out.push(RawNode {
                        name,
                        node_type: NodeType::Struct, // class -> struct
                        content: node.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        line_start: node.start_position().row,
                        line_end: node.end_position().row,
                    });
                }
                // Also extract methods inside class
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for child in body.children(&mut cursor) {
                        if child.kind() == "function_definition" {
                            if let Some(n) = child.child_by_field_name("name") {
                                let name =
                                    n.utf8_text(source.as_bytes()).unwrap_or("").to_string();
                                out.push(RawNode {
                                    name,
                                    node_type: NodeType::Method,
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
            }
            _ => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    Self::walk(child, source, out);
                }
            }
        }
    }

    /// Extract imported names.
    /// `from foo import bar` → "bar"
    /// `import os` → "os"
    /// `from x import a, b as c` → "a", "c"
    fn walk_imports(node: tree_sitter::Node, source: &str, out: &mut Vec<String>) {
        match node.kind() {
            "import_from_statement" => {
                // from X import a, b, c
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "dotted_name" || child.kind() == "identifier" {
                        // Skip the module name (first dotted_name is the source)
                        // Names after "import" keyword are the imports
                    }
                    if child.kind() == "aliased_import" {
                        // `a as b` → use the alias
                        if let Some(alias) = child.child_by_field_name("alias") {
                            let name = alias.utf8_text(source.as_bytes()).unwrap_or("");
                            if !name.is_empty() {
                                out.push(name.to_string());
                            }
                        } else if let Some(name_node) = child.child_by_field_name("name") {
                            let name = name_node.utf8_text(source.as_bytes()).unwrap_or("");
                            if !name.is_empty() {
                                out.push(name.to_string());
                            }
                        }
                    }
                }
                // Fallback: parse the text directly
                let text = node.utf8_text(source.as_bytes()).unwrap_or("");
                if let Some(import_part) = text.split("import").nth(1) {
                    for item in import_part.split(',') {
                        let name = item.trim().split(" as ").last().unwrap_or("").trim();
                        if !name.is_empty() && name != "*" {
                            if !out.contains(&name.to_string()) {
                                out.push(name.to_string());
                            }
                        }
                    }
                }
            }
            "import_statement" => {
                // import os, sys
                let text = node.utf8_text(source.as_bytes()).unwrap_or("");
                let after = text.trim_start_matches("import").trim();
                for item in after.split(',') {
                    let name = item.trim().split(" as ").last().unwrap_or("").trim();
                    let short = name.rsplit('.').next().unwrap_or(name);
                    if !short.is_empty() {
                        out.push(short.to_string());
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
        if node.kind() == "call" {
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
