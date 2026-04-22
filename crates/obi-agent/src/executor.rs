use anyhow::Result;

use obi_llm::provider::ToolCall;

use crate::tools::{ToolPermission, ToolRegistry, ToolResult};

/// Outcome of attempting to execute a tool call.
#[derive(Debug)]
pub enum ExecutionOutcome {
    /// Tool executed successfully.
    Completed(ToolResult),
    /// Tool requires user confirmation before execution.
    /// The TUI should display a confirmation dialog.
    NeedsConfirmation {
        tool_name: String,
        description: String,
        args_display: String,
        call: ToolCall,
    },
    /// Tool not found in registry.
    NotFound(String),
}

/// Tool call executor with safety checks.
/// Handles permission levels and delegates to the appropriate tool.
pub struct Executor;

impl Executor {
    /// Attempt to execute a tool call.
    /// Returns NeedsConfirmation for Mutating/Dangerous tools unless pre-approved.
    pub async fn execute(
        registry: &ToolRegistry,
        call: &ToolCall,
        approved: bool,
    ) -> Result<ExecutionOutcome> {
        let tool = match registry.get(&call.name) {
            Some(t) => t,
            None => {
                return Ok(ExecutionOutcome::NotFound(format!(
                    "Unknown tool: '{}'",
                    call.name
                )));
            }
        };

        let args: serde_json::Value = serde_json::from_str(&call.arguments)
            .unwrap_or(serde_json::Value::Object(Default::default()));

        // Check permission level
        match tool.permission() {
            ToolPermission::ReadOnly => {
                // Always allowed
                let result = tool.execute(args).await?;
                Ok(ExecutionOutcome::Completed(result))
            }
            ToolPermission::Mutating | ToolPermission::Dangerous => {
                if approved {
                    let result = tool.execute(args).await?;
                    Ok(ExecutionOutcome::Completed(result))
                } else {
                    let description = match tool.permission() {
                        ToolPermission::Mutating => {
                            format!("AI wants to modify a file using '{}'", call.name)
                        }
                        ToolPermission::Dangerous => {
                            format!("AI wants to execute: {}", format_args_for_display(&call.name, &args))
                        }
                        _ => unreachable!(),
                    };

                    Ok(ExecutionOutcome::NeedsConfirmation {
                        tool_name: call.name.clone(),
                        description,
                        args_display: format_args_for_display(&call.name, &args),
                        call: call.clone(),
                    })
                }
            }
        }
    }
}

/// Format tool arguments for human-readable display in confirmation dialogs.
fn format_args_for_display(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "run" => {
            args.get("command")
                .and_then(|v| v.as_str())
                .map(|cmd| format!("$ {}", cmd))
                .unwrap_or_else(|| args.to_string())
        }
        "edit" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            format!("edit {}", path)
        }
        _ => {
            serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_args_run() {
        let args = serde_json::json!({"command": "cargo test"});
        assert_eq!(format_args_for_display("run", &args), "$ cargo test");
    }

    #[test]
    fn test_format_args_edit() {
        let args = serde_json::json!({"path": "src/main.rs", "old_string": "a", "new_string": "b"});
        assert_eq!(format_args_for_display("edit", &args), "edit src/main.rs");
    }
}
