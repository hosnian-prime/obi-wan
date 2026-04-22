use serde::{Deserialize, Serialize};
use std::ops::Range;
use std::path::PathBuf;
use uuid::Uuid;

pub type NodeId = Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeType {
    Function,
    Method,
    Struct,
    Trait,
    Constant,
    File,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    Unknown,
}

/// Lightweight in-memory graph node — content and embeddings live in LanceDB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticNode {
    pub id: NodeId,
    pub node_type: NodeType,
    pub name: String,
    pub file_path: PathBuf,
    pub line_range: Range<usize>,
    pub language: Language,
    /// blake3 hash of the node's source content — used for incremental indexing.
    pub content_hash: [u8; 32],
}

impl SemanticNode {
    pub fn new(
        node_type: NodeType,
        name: String,
        file_path: PathBuf,
        line_range: Range<usize>,
        language: Language,
        content: &str,
    ) -> Self {
        let content_hash = blake3::hash(content.as_bytes()).into();
        Self {
            id: Uuid::new_v4(),
            node_type,
            name,
            file_path,
            line_range,
            language,
            content_hash,
        }
    }
}
