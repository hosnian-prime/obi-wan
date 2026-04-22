use std::path::Path;

use obi_core::node::{Language, NodeType};
use tree_sitter::Parser;

use super::{CallSite, LanguageParser, RawNode};

/// Handles both TypeScript and JavaScript files.
pub struct TypeScriptParser;

impl LanguageParser for TypeScriptParser {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn file_extensions(&self) -> &[&str] {
        &["ts", "tsx", "js", "jsx"]
    }

    fn extract_nodes(&self, source: &str, path: &Path) -> Vec<RawNode> {
        let (nodes, _tree) = self.parse_with_tree(source, path, None);
        nodes
    }

    fn extract_nodes_incremental(
        &self,
        source: &str,
        path: &Path,
        old_tree: Option<&tree_sitter::Tree>,
    ) -> (Vec<RawNode>, Option<tree_sitter::Tree>) {
        self.parse_with_tree(source, path, old_tree)
    }

    fn resolve_imports(&self, source: &str) -> Vec<String> {
        let mut parser = Parser::new();
        let lang = tree_sitter_typescript::LANGUAGE_TYPESCRIPT;
        parser
            .set_language(&lang.into())
            .expect("failed to set TS grammar");

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
        let lang = tree_sitter_typescript::LANGUAGE_TYPESCRIPT;
        parser
            .set_language(&lang.into())
            .expect("failed to set TS grammar");

        let tree = match parser.parse(source, None) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut calls = Vec::new();
        Self::walk_calls(tree.root_node(), source, &mut calls);
        calls
    }
}

impl TypeScriptParser {
    fn parse_with_tree(
        &self,
        source: &str,
        path: &Path,
        old_tree: Option<&tree_sitter::Tree>,
    ) -> (Vec<RawNode>, Option<tree_sitter::Tree>) {
        let mut parser = Parser::new();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("ts");
        let lang = match ext {
            "tsx" => tree_sitter_typescript::LANGUAGE_TSX,
            "js" | "jsx" => tree_sitter_javascript::LANGUAGE,
            _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        };
        parser
            .set_language(&lang.into())
            .expect("failed to set TS/JS grammar");

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
            "function_declaration" | "generator_function_declaration" => {
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
            "class_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(RawNode {
                        name: name.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        node_type: NodeType::Struct,
                        content: node.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        line_start: node.start_position().row,
                        line_end: node.end_position().row,
                    });
                }
                // Extract methods
                if let Some(body) = node.child_by_field_name("body") {
                    let mut cursor = body.walk();
                    for child in body.children(&mut cursor) {
                        if child.kind() == "method_definition" {
                            if let Some(n) = child.child_by_field_name("name") {
                                out.push(RawNode {
                                    name: n
                                        .utf8_text(source.as_bytes())
                                        .unwrap_or("")
                                        .to_string(),
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
            "interface_declaration" | "type_alias_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.push(RawNode {
                        name: name.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        node_type: NodeType::Trait,
                        content: node.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                        line_start: node.start_position().row,
                        line_end: node.end_position().row,
                    });
                }
            }
            // Arrow functions assigned to const/let/var
            "lexical_declaration" | "variable_declaration" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "variable_declarator" {
                        let has_arrow = child
                            .child_by_field_name("value")
                            .map(|v| v.kind() == "arrow_function")
                            .unwrap_or(false);
                        if has_arrow {
                            if let Some(name) = child.child_by_field_name("name") {
                                out.push(RawNode {
                                    name: name
                                        .utf8_text(source.as_bytes())
                                        .unwrap_or("")
                                        .to_string(),
                                    node_type: NodeType::Function,
                                    content: node
                                        .utf8_text(source.as_bytes())
                                        .unwrap_or("")
                                        .to_string(),
                                    line_start: node.start_position().row,
                                    line_end: node.end_position().row,
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
    /// `import { foo, bar } from './baz'` → "foo", "bar"
    /// `import React from 'react'` → "React"
    /// `import * as fs from 'fs'` → "fs"
    fn walk_imports(node: tree_sitter::Node, source: &str, out: &mut Vec<String>) {
        if node.kind() == "import_statement" {
            let text = node.utf8_text(source.as_bytes()).unwrap_or("");
            // Named imports: import { a, b as c } from '...'
            if let Some(brace_start) = text.find('{') {
                if let Some(brace_end) = text.find('}') {
                    let inner = &text[brace_start + 1..brace_end];
                    for item in inner.split(',') {
                        let name = item.trim().split(" as ").last().unwrap_or("").trim();
                        if !name.is_empty() {
                            out.push(name.to_string());
                        }
                    }
                }
            }
            // Default import: import React from '...'
            // Namespace import: import * as fs from '...'
            else if let Some(from_idx) = text.find(" from ") {
                let before_from = text[..from_idx].trim_start_matches("import").trim();
                if before_from.starts_with("* as ") {
                    let name = before_from.trim_start_matches("* as ").trim();
                    if !name.is_empty() {
                        out.push(name.to_string());
                    }
                } else if !before_from.is_empty() && !before_from.starts_with("type ") {
                    out.push(before_from.to_string());
                }
            }
        } else {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                Self::walk_imports(child, source, out);
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
