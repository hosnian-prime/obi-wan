use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use obi_core::graph::KnowledgeGraph;
use super::{Tool, ToolPermission, ToolResult};

/// Query the knowledge graph — neighbors, paths, subgraphs.
pub struct GraphQueryTool {
    graph: Option<Arc<KnowledgeGraph>>,
}

impl GraphQueryTool {
    pub fn new(graph: Option<Arc<KnowledgeGraph>>) -> Self {
        Self { graph }
    }
}

#[async_trait]
impl Tool for GraphQueryTool {
    fn name(&self) -> &str {
        "graph_query"
    }

    fn description(&self) -> &str {
        "Query the knowledge graph. Available operations:\n\
         - 'neighbors': Get nodes connected to a given node (by name). Returns connected nodes with edge types.\n\
         - 'info': Get information about a specific node (by name).\n\
         - 'stats': Get overall graph statistics (node count, edge count, type breakdown)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["operation"],
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["neighbors", "info", "stats"],
                    "description": "The graph operation to perform"
                },
                "node_name": {
                    "type": "string",
                    "description": "Node name to query (required for 'neighbors' and 'info')"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Max depth for neighbor traversal (default: 1)"
                }
            }
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let graph = match &self.graph {
            Some(g) => g,
            None => {
                return Ok(ToolResult {
                    output: "Knowledge graph not loaded. Run `obi index` first.".to_string(),
                    success: false,
                });
            }
        };

        let operation = args
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'operation' argument"))?;

        match operation {
            "stats" => {
                let node_count = graph.node_count();
                let edge_count = graph.edge_count();

                // Type breakdown
                let mut type_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for node in graph.nodes() {
                    *type_counts
                        .entry(format!("{:?}", node.node_type))
                        .or_insert(0) += 1;
                }

                let mut breakdown: Vec<String> = type_counts
                    .iter()
                    .map(|(t, c)| format!("  {}: {}", t, c))
                    .collect();
                breakdown.sort();

                Ok(ToolResult {
                    output: format!(
                        "Graph statistics:\n  Nodes: {}\n  Edges: {}\n\nBy type:\n{}",
                        node_count,
                        edge_count,
                        breakdown.join("\n")
                    ),
                    success: true,
                })
            }
            "info" => {
                let node_name = args
                    .get("node_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("'node_name' required for 'info' operation"))?;

                let node = find_node_by_name(graph, node_name);
                match node {
                    Some(n) => Ok(ToolResult {
                        output: format!(
                            "Node: {}\n  Type: {:?}\n  File: {}:{}-{}\n  Language: {:?}\n  Hash: {}",
                            n.name,
                            n.node_type,
                            n.file_path.display(),
                            n.line_range.start,
                            n.line_range.end,
                            n.language,
                            n.content_hash.iter().map(|b| format!("{:02x}", b)).collect::<String>()
                        ),
                        success: true,
                    }),
                    None => Ok(ToolResult {
                        output: format!("No node found with name '{}'", node_name),
                        success: false,
                    }),
                }
            }
            "neighbors" => {
                let node_name = args
                    .get("node_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("'node_name' required for 'neighbors' operation")
                    })?;

                let max_depth = args
                    .get("max_depth")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as usize;

                let node = find_node_by_name(graph, node_name);
                let node_id = match node {
                    Some(n) => n.id,
                    None => {
                        return Ok(ToolResult {
                            output: format!("No node found with name '{}'", node_name),
                            success: false,
                        });
                    }
                };

                let neighbors = graph.neighbors(&node_id, max_depth);
                if neighbors.is_empty() {
                    return Ok(ToolResult {
                        output: format!("Node '{}' has no neighbors within depth {}", node_name, max_depth),
                        success: true,
                    });
                }

                let mut lines: Vec<String> = Vec::new();
                lines.push(format!("Neighbors of '{}' (depth {}):", node_name, max_depth));
                for (neighbor_id, edge, depth) in &neighbors {
                    let neighbor_name = graph
                        .get_node(neighbor_id)
                        .map(|n| n.name.as_str())
                        .unwrap_or("?");
                    let direction = if edge.source == node_id {
                        "→"
                    } else {
                        "←"
                    };
                    lines.push(format!(
                        "  {} {} (d{}) {:?} w={:.2}",
                        direction, neighbor_name, depth, edge.edge_type, edge.weight
                    ));
                }

                Ok(ToolResult {
                    output: lines.join("\n"),
                    success: true,
                })
            }
            _ => Ok(ToolResult {
                output: format!("Unknown operation '{}'. Use: neighbors, info, stats", operation),
                success: false,
            }),
        }
    }
}

fn find_node_by_name<'a>(
    graph: &'a KnowledgeGraph,
    name: &str,
) -> Option<&'a obi_core::node::SemanticNode> {
    graph.nodes().find(|n| n.name == name)
}
