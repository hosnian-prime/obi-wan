pub mod differ;
pub mod edges;
pub mod embedder;
pub mod notes;
pub mod parser;
pub mod store;
pub mod watcher;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use obi_core::edge::{Edge, EdgeType};
use obi_core::graph::KnowledgeGraph;
use obi_core::node::{NodeId, SemanticNode};

use crate::parser::{parser_for_path, CallSite};

/// Index a directory, building a KnowledgeGraph from all supported source files.
pub fn index_directory(root: &Path, config: &IndexConfig) -> Result<IndexResult> {
    let mut graph = KnowledgeGraph::new();
    let mut node_contents: HashMap<NodeId, String> = HashMap::new();
    let mut all_call_sites: Vec<(NodeId, Vec<CallSite>)> = Vec::new();
    let mut tree_cache: HashMap<PathBuf, tree_sitter::Tree> = HashMap::new();

    // Per-file data for import resolution
    let mut file_imports: Vec<(Vec<NodeId>, Vec<String>)> = Vec::new();
    // Wiki links resolved separately as UserLink edges
    let mut wiki_link_edges: Vec<(NodeId, Vec<String>)> = Vec::new();

    let files = collect_source_files(root, &config.exclude);

    for file_path in &files {
        let source = match std::fs::read_to_string(file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Check if it's a markdown note
        if file_path.extension().map(|e| e == "md").unwrap_or(false) {
            let file_name = file_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let (raw_node, wiki_links) = notes::parse_note(&source, &file_name);
            let node = SemanticNode::new(
                raw_node.node_type,
                raw_node.name,
                file_path.clone(),
                raw_node.line_start..raw_node.line_end,
                obi_core::node::Language::Unknown,
                &raw_node.content,
            );
            let node_id = node.id;
            node_contents.insert(node_id, raw_node.content);
            graph.add_node(node);

            // Collect wiki links for UserLink edge resolution (not StaticCall)
            if !wiki_links.is_empty() {
                wiki_link_edges.push((node_id, wiki_links));
            }
            continue;
        }

        let parser = parser_for_path(file_path);

        // Incremental parsing with tree cache
        let old_tree = tree_cache.get(file_path);
        let (raw_nodes, new_tree) =
            parser.extract_nodes_incremental(&source, file_path, old_tree);

        if let Some(tree) = new_tree {
            tree_cache.insert(file_path.clone(), tree);
        }

        let calls = parser.resolve_calls(&source);
        let imports = parser.resolve_imports(&source);

        let mut file_node_ids = Vec::new();

        for raw_node in &raw_nodes {
            let node = SemanticNode::new(
                raw_node.node_type,
                raw_node.name.clone(),
                file_path.clone(),
                raw_node.line_start..raw_node.line_end,
                parser.language(),
                &raw_node.content,
            );
            let node_id = node.id;
            file_node_ids.push(node_id);
            node_contents.insert(node_id, raw_node.content.clone());
            graph.add_node(node);

            // Collect call sites that originate from this node's line range
            let node_calls: Vec<CallSite> = calls
                .iter()
                .filter(|c| {
                    c.caller_line >= raw_node.line_start && c.caller_line <= raw_node.line_end
                })
                .cloned()
                .collect();
            if !node_calls.is_empty() {
                all_call_sites.push((node_id, node_calls));
            }
        }

        // Collect imports for this file's nodes
        if !imports.is_empty() && !file_node_ids.is_empty() {
            file_imports.push((file_node_ids, imports));
        }
    }

    // 1. Resolve static call edges
    let static_edges = edges::resolve_static_edges(graph.all_nodes(), &all_call_sites);
    for edge in static_edges {
        graph.add_edge(edge);
    }

    // 2. Resolve import edges
    for (file_node_ids, imported_names) in &file_imports {
        let import_edges =
            edges::resolve_import_edges(graph.all_nodes(), file_node_ids, imported_names);
        for edge in import_edges {
            graph.add_edge(edge);
        }
    }

    // 3. Resolve wiki link edges as UserLink (weight: 1.0)
    let user_link_edges: Vec<Edge> = {
        let name_to_ids: HashMap<&str, Vec<NodeId>> = {
            let mut map: HashMap<&str, Vec<NodeId>> = HashMap::new();
            for (id, node) in graph.all_nodes() {
                map.entry(node.name.as_str()).or_default().push(*id);
            }
            map
        };
        let mut edges = Vec::new();
        for (source_id, link_targets) in &wiki_link_edges {
            for target_name in link_targets {
                if let Some(target_ids) = name_to_ids.get(target_name.as_str()) {
                    for &target_id in target_ids {
                        if target_id != *source_id {
                            edges.push(Edge::new(*source_id, target_id, EdgeType::UserLink));
                        }
                    }
                }
            }
        }
        edges
    };
    for edge in user_link_edges {
        graph.add_edge(edge);
    }

    // 4. Resolve type reference edges
    let type_ref_edges = edges::resolve_type_ref_edges(graph.all_nodes(), &node_contents);
    for edge in type_ref_edges {
        graph.add_edge(edge);
    }

    Ok(IndexResult {
        graph,
        node_contents,
        files_indexed: files.len(),
    })
}

/// Collect all source files under root, respecting exclusions.
fn collect_source_files(root: &Path, exclude: &[String]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_recursive(root, exclude, &mut files);
    files
}

fn collect_recursive(dir: &Path, exclude: &[String], out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden and excluded
        if name.starts_with('.') || exclude.iter().any(|ex| name == ex.trim_end_matches('/')) {
            continue;
        }

        if path.is_dir() {
            collect_recursive(&path, exclude, out);
        } else if is_supported_file(&path) {
            out.push(path);
        }
    }
}

fn is_supported_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "go" | "md" | "toml" | "json" | "yaml"
            | "yml" | "c" | "cpp" | "h" | "hpp" | "java" | "rb" | "sh"
    )
}

pub struct IndexConfig {
    pub exclude: Vec<String>,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            exclude: vec![
                "target".into(),
                "node_modules".into(),
                ".git".into(),
                ".obi".into(),
            ],
        }
    }
}

pub struct IndexResult {
    pub graph: KnowledgeGraph,
    pub node_contents: HashMap<NodeId, String>,
    pub files_indexed: usize,
}
