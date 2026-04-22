use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolPermission, ToolResult};

/// Apply code edits to a file — find-and-replace based.
pub struct EditTool {
    project_root: PathBuf,
}

impl EditTool {
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
impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing an exact string match with new content. \
         Provide the file path, the old string to find, and the new string to replace it with. \
         The old_string must match exactly (including whitespace and indentation)."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "old_string", "new_string"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to project root"
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact string to find in the file"
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement string"
                }
            }
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Mutating
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'path' argument"))?;
        let old_string = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'old_string' argument"))?;
        let new_string = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'new_string' argument"))?;

        let full_path = self.resolve_path(path);

        // Security: ensure path is within project root
        let canonical = match full_path.canonicalize() {
            Ok(p) => p,
            Err(e) => {
                return Ok(ToolResult {
                    output: format!("Error: cannot access '{}': {}", path, e),
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

        let match_count = content.matches(old_string).count();
        if match_count == 0 {
            return Ok(ToolResult {
                output: format!(
                    "Error: old_string not found in '{}'. Make sure it matches exactly.",
                    path
                ),
                success: false,
            });
        }
        if match_count > 1 {
            return Ok(ToolResult {
                output: format!(
                    "Error: old_string found {} times in '{}'. It must be unique. \
                     Provide more surrounding context to make it unique.",
                    match_count, path
                ),
                success: false,
            });
        }

        let new_content = content.replacen(old_string, new_string, 1);

        if let Err(e) = tokio::fs::write(&canonical, &new_content).await {
            return Ok(ToolResult {
                output: format!("Error writing '{}': {}", path, e),
                success: false,
            });
        }

        Ok(ToolResult {
            output: format!("Successfully edited '{}'", path),
            success: true,
        })
    }
}
