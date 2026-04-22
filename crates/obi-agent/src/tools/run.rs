use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use super::command::ParsedCommand;
use super::security::{SecurityPolicy, SecurityVerdict};
use super::{Tool, ToolPermission, ToolResult};

/// Execute commands within the project directory.
///
/// Execution strategy:
/// - Simple commands (e.g. `cargo build --release`) → direct binary execution (no shell).
/// - Commands with shell features (pipes, redirects, chaining) → shell fallback.
///
/// Direct execution avoids spawning `sh -c`, which:
/// - Eliminates shell injection vectors
/// - Produces cleaner process trees (EDR-friendly)
/// - Reduces attack surface
pub struct RunTool {
    project_root: PathBuf,
}

impl RunTool {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

/// Command timeout in seconds.
const COMMAND_TIMEOUT_SECS: u64 = 30;

/// Max output size in bytes to prevent context overflow.
const MAX_OUTPUT_BYTES: usize = 8_000;

#[async_trait]
impl Tool for RunTool {
    fn name(&self) -> &str {
        "run"
    }

    fn description(&self) -> &str {
        "Execute a command in the project directory. \
         The command runs with a 30-second timeout. \
         Use this for tasks like running tests, building, or checking output. \
         No sudo or privilege escalation is allowed."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Command to execute (e.g. 'cargo test', 'npm run build')"
                }
            }
        })
    }

    fn permission(&self) -> ToolPermission {
        ToolPermission::Dangerous
    }

    async fn execute(&self, args: Value) -> Result<ToolResult> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'command' argument"))?;

        // Step 1: Security validation
        if let SecurityVerdict::Block(reason) = SecurityPolicy::validate_raw(command) {
            return Ok(ToolResult {
                output: format!("Error: {reason}"),
                success: false,
            });
        }

        // Step 2: Parse command (direct exec vs shell fallback)
        let parsed = ParsedCommand::parse(command);

        // Step 3: Execute with timeout
        match parsed.execute(&self.project_root, COMMAND_TIMEOUT_SECS).await {
            Ok(output) => Ok(ToolResult {
                output: output.format(MAX_OUTPUT_BYTES),
                success: output.success,
            }),
            Err(e) => Ok(ToolResult {
                output: format!("Error: {e}"),
                success: false,
            }),
        }
    }
}
