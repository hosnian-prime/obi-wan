use std::path::Path;

use anyhow::Result;

/// Parsed command ready for execution — either direct (no shell) or via shell fallback.
#[derive(Debug, Clone)]
pub enum ParsedCommand {
    /// Direct execution: binary + args. No shell intermediary.
    /// EDR-friendly: process tree shows `your_app → cargo` instead of `your_app → sh → cargo`.
    Direct {
        program: String,
        args: Vec<String>,
    },
    /// Shell fallback for commands that require shell features (pipes, redirects, chaining).
    /// Used only when direct execution is not possible.
    Shell {
        raw: String,
    },
}

/// Shell metacharacters that require shell interpretation.
/// If a command contains any of these, it must go through shell fallback.
const SHELL_META: &[&str] = &["|", "&&", "||", ";", ">", ">>", "<", "<<", "`", "$(", "${", "*", "?", "~"];

impl ParsedCommand {
    /// Parse a raw command string into a `ParsedCommand`.
    ///
    /// Strategy:
    /// 1. Check for shell metacharacters → Shell fallback
    /// 2. Split using POSIX shell quoting rules → Direct execution
    /// 3. If splitting fails → Shell fallback
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();

        // Check for shell features that require shell interpretation
        if requires_shell(trimmed) {
            return Self::Shell {
                raw: trimmed.to_string(),
            };
        }

        // Try POSIX-compliant token splitting (handles quotes, escapes)
        match shell_words::split(trimmed) {
            Ok(tokens) if !tokens.is_empty() => Self::Direct {
                program: tokens[0].clone(),
                args: tokens[1..].to_vec(),
            },
            // Malformed quoting or empty — fall back to shell
            _ => Self::Shell {
                raw: trimmed.to_string(),
            },
        }
    }

    /// Execute the command with timeout. Returns (stdout+stderr, exit_code).
    pub async fn execute(
        &self,
        working_dir: &Path,
        timeout_secs: u64,
    ) -> Result<CommandOutput> {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            self.spawn(working_dir),
        )
        .await;

        match output {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                Ok(CommandOutput {
                    stdout: stdout.into_owned(),
                    stderr: stderr.into_owned(),
                    exit_code,
                    success: output.status.success(),
                })
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("failed to execute command: {}", e)),
            Err(_) => Err(anyhow::anyhow!(
                "command timed out after {} seconds",
                timeout_secs
            )),
        }
    }

    /// Spawn the actual process.
    async fn spawn(&self, working_dir: &Path) -> std::io::Result<std::process::Output> {
        match self {
            Self::Direct { program, args } => {
                tokio::process::Command::new(program)
                    .args(args)
                    .current_dir(working_dir)
                    .output()
                    .await
            }
            Self::Shell { raw } => {
                tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(raw)
                    .current_dir(working_dir)
                    .output()
                    .await
            }
        }
    }

    /// Human-readable display for confirmation dialogs.
    pub fn display(&self) -> String {
        match self {
            Self::Direct { program, args } => {
                if args.is_empty() {
                    format!("$ {program}")
                } else {
                    format!("$ {program} {}", args.join(" "))
                }
            }
            Self::Shell { raw } => format!("$ {raw}"),
        }
    }

    /// Whether this command uses shell fallback.
    pub fn uses_shell(&self) -> bool {
        matches!(self, Self::Shell { .. })
    }
}

/// Output from a command execution.
#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub success: bool,
}

impl CommandOutput {
    /// Format output for tool result, with optional truncation.
    pub fn format(&self, max_bytes: usize) -> String {
        let mut result = String::new();

        if !self.stdout.is_empty() {
            result.push_str(&self.stdout);
        }
        if !self.stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str("[stderr]\n");
            result.push_str(&self.stderr);
        }

        if result.len() > max_bytes {
            result.truncate(max_bytes);
            result.push_str("\n... (output truncated)");
        }

        if self.exit_code != 0 {
            result.push_str(&format!("\n[exit code: {}]", self.exit_code));
        }

        if result.is_empty() {
            "(no output)".to_string()
        } else {
            result
        }
    }
}

/// Check whether a command string contains shell metacharacters
/// that require shell interpretation.
fn requires_shell(cmd: &str) -> bool {
    SHELL_META.iter().any(|meta| cmd.contains(meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_command_is_direct() {
        let parsed = ParsedCommand::parse("cargo build --release");
        match parsed {
            ParsedCommand::Direct { program, args } => {
                assert_eq!(program, "cargo");
                assert_eq!(args, vec!["build", "--release"]);
            }
            _ => panic!("expected Direct"),
        }
    }

    #[test]
    fn quoted_args_are_direct() {
        let parsed = ParsedCommand::parse(r#"echo "hello world" foo"#);
        match parsed {
            ParsedCommand::Direct { program, args } => {
                assert_eq!(program, "echo");
                assert_eq!(args, vec!["hello world", "foo"]);
            }
            _ => panic!("expected Direct"),
        }
    }

    #[test]
    fn pipe_requires_shell() {
        let parsed = ParsedCommand::parse("cat file.txt | grep pattern");
        assert!(parsed.uses_shell());
    }

    #[test]
    fn and_chain_requires_shell() {
        let parsed = ParsedCommand::parse("cargo build && cargo test");
        assert!(parsed.uses_shell());
    }

    #[test]
    fn redirect_requires_shell() {
        let parsed = ParsedCommand::parse("echo hello > output.txt");
        assert!(parsed.uses_shell());
    }

    #[test]
    fn glob_requires_shell() {
        let parsed = ParsedCommand::parse("ls *.rs");
        assert!(parsed.uses_shell());
    }

    #[test]
    fn command_substitution_requires_shell() {
        let parsed = ParsedCommand::parse("echo $(date)");
        assert!(parsed.uses_shell());
    }

    #[test]
    fn simple_npm_is_direct() {
        let parsed = ParsedCommand::parse("npm test");
        match parsed {
            ParsedCommand::Direct { program, args } => {
                assert_eq!(program, "npm");
                assert_eq!(args, vec!["test"]);
            }
            _ => panic!("expected Direct"),
        }
    }

    #[test]
    fn display_format() {
        let direct = ParsedCommand::parse("cargo test --lib");
        assert_eq!(direct.display(), "$ cargo test --lib");

        let shell = ParsedCommand::parse("cargo build && cargo test");
        assert_eq!(shell.display(), "$ cargo build && cargo test");
    }

    #[test]
    fn empty_command_is_shell_fallback() {
        let parsed = ParsedCommand::parse("");
        assert!(parsed.uses_shell());
    }
}
