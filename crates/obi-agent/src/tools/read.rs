use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolPermission, ToolResult};

/// Read file or node content.
pub struct ReadTool {
    project_root: PathBuf,
}

impl ReadTool {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.project_root.join(p)
        }
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Provide a file path relative to the project root. \
         Optionally specify start_line and end_line to read a specific range."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to project root"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Start line number (1-based, inclusive)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "End line number (1-based, inclusive)"
                }
            }
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::ReadOnly
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'path' argument"))?;

        let full_path = self.resolve_path(path);

        // Security: ensure path is within project root
        let canonical = match full_path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("Error: cannot read '{}': {}", path, e),
                    success: false,
                });
            }
        };
        let root_canonical = self.project_root.canonicalize().unwrap_or(self.project_root.clone());
        if !canonical.starts_with(&root_canonical) {
            return Ok(ToolResult {
                output: format!("Error: path '{}' is outside the project directory", path),
                success: false,
            });
        }

        let content = match tokio::fs::read_to_string(&canonical).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("Error reading '{}': {}", path, e),
                    success: false,
                });
            }
        };

        let start_line = args.get("start_line").and_then(|v| v.as_u64()).map(|v| v as usize);
        let end_line = args.get("end_line").and_then(|v| v.as_u64()).map(|v| v as usize);

        let output = match (start_line, end_line) {
            (Some(start), Some(end)) => {
                let lines: Vec<&str> = content.lines().collect();
                let start = start.saturating_sub(1).min(lines.len());
                let end = end.min(lines.len());
                lines[start..end]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:4} | {}", start + i + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            (Some(start), None) => {
                let lines: Vec<&str> = content.lines().collect();
                let start = start.saturating_sub(1).min(lines.len());
                lines[start..]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:4} | {}", start + i + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => content,
        };

        // Truncate very large outputs to prevent context overflow
        let max_chars = 10_000;
        let output = if output.len() > max_chars {
            format!(
                "{}\n\n... (truncated, {} total chars)",
                &output[..max_chars],
                output.len()
            )
        } else {
            output
        };

        Ok(ToolResult {
            output,
            success: true,
        })
    }
}
