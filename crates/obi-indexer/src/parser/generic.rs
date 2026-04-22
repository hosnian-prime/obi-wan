use std::path::Path;

use obi_core::node::{Language, NodeType};

use super::{CallSite, LanguageParser, RawNode};

const CHUNK_SIZE: usize = 50;

/// Fallback parser for unsupported languages. Chunks file into ~50-line blocks.
pub struct GenericParser;

impl LanguageParser for GenericParser {
    fn language(&self) -> Language {
        Language::Unknown
    }

    fn file_extensions(&self) -> &[&str] {
        &[] // fallback, no specific extensions
    }

    fn extract_nodes(&self, source: &str, path: &Path) -> Vec<RawNode> {
        let lines: Vec<&str> = source.lines().collect();
        let file_name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "chunk".into());

        lines
            .chunks(CHUNK_SIZE)
            .enumerate()
            .map(|(i, chunk)| {
                let start = i * CHUNK_SIZE;
                let end = start + chunk.len();
                RawNode {
                    name: format!("{}_{}", file_name, i),
                    node_type: NodeType::File,
                    content: chunk.join("\n"),
                    line_start: start,
                    line_end: end,
                }
            })
            .collect()
    }

    fn resolve_calls(&self, _source: &str) -> Vec<CallSite> {
        Vec::new() // no call resolution for generic chunks
    }
}
