use crossterm::event::{KeyEvent, MouseEvent};
use std::path::PathBuf;

use obi_agent::context::ContextWindow;
use obi_core::node::NodeId;

/// Unified event enum — all subsystems communicate through this channel.
/// Cross-ref: docs/02-architecture.md AppEvent enum
#[derive(Debug)]
#[allow(dead_code)]
pub enum AppEvent {
    // TUI
    Key(KeyEvent),
    Mouse(MouseEvent),
    Resize(u16, u16),

    // Indexer (Phase 2)
    NodesUpdated(Vec<NodeId>),
    IndexingProgress { done: usize, total: usize },
    IndexingComplete,
    IndexingError(String),
    IndexingStatus(String),

    // Agent (Phase 5)
    StreamChunk(String),
    ThinkingChunk(String),
    ResponseComplete(String),
    PlanGenerated(String),
    ToolConfirmation {
        tool_name: String,
        description: String,
        args_display: String,
        call_id: String,
    },
    AgentError(String),
    ModelInfo {
        provider: String,
        chat_model: String,
        embed_model: String,
    },

    // Context builder (Phase 4)
    ContextBuilt(ContextWindow),
    ContextError(String),
    ContextBuilding,

    // Graph layout (Phase 3)
    LayoutUpdated,

    // File watcher (Phase 2)
    FileChanged(Vec<PathBuf>),

    // Settings
    /// Fetched model list from Anthropic API.
    ModelListFetched(Vec<String>),
    /// Fetched model list from Z.ai API.
    ZaiModelListFetched(Vec<String>),

    // Internal
    Quit,
}
