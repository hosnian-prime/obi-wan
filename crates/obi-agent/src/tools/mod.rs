pub mod command;
pub mod edit;
pub mod graph;
pub mod read;
pub mod run;
pub mod search;
pub mod security;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use obi_llm::provider::ToolDefinition;
use serde_json::Value;

/// Permission level for tool execution.
/// Controls whether user confirmation is needed before running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPermission {
    /// Always allowed, no confirmation needed (read, search, graph_query).
    ReadOnly,
    /// Requires user confirmation in TUI before execution (edit).
    Mutating,
    /// Requires explicit user confirmation + shows full command (run).
    Dangerous,
}

/// Result of a tool execution.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub success: bool,
}

/// Tool trait — each agent tool implements this.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (used in LLM function calling).
    fn name(&self) -> &str;

    /// Human-readable description for the LLM.
    fn description(&self) -> &str;

    /// JSON Schema for the tool's parameters.
    fn parameters_schema(&self) -> Value;

    /// Permission level.
    fn permission(&self) -> ToolPermission;

    /// Execute the tool with the given JSON arguments.
    async fn execute(&self, args: Value) -> Result<ToolResult>;

    /// Convert to LLM ToolDefinition for the completion API.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

/// Tool registry — holds all available tools, provides lookup by name.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new registry with all default tools for the given project root.
    pub fn new(
        project_root: PathBuf,
        knowledge_graph: Option<Arc<obi_core::graph::KnowledgeGraph>>,
    ) -> Self {
        let mut tools: HashMap<String, Box<dyn Tool>> = HashMap::new();

        let read_tool = read::ReadTool::new(project_root.clone());
        tools.insert(read_tool.name().to_string(), Box::new(read_tool));

        let edit_tool = edit::EditTool::new(project_root.clone());
        tools.insert(edit_tool.name().to_string(), Box::new(edit_tool));

        let search_tool = search::SearchTool::new(project_root.clone());
        tools.insert(search_tool.name().to_string(), Box::new(search_tool));

        let run_tool = run::RunTool::new(project_root.clone());
        tools.insert(run_tool.name().to_string(), Box::new(run_tool));

        let graph_tool = graph::GraphQueryTool::new(knowledge_graph);
        tools.insert(graph_tool.name().to_string(), Box::new(graph_tool));

        Self { tools }
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    /// Get all tool definitions for the LLM.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.to_definition()).collect()
    }

    /// List all tool names.
    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(|s| s.as_str()).collect()
    }
}
