/// Command security policy — validates commands before execution.
///
/// Single Responsibility: all security rules live here, not scattered across tools.
/// Open/Closed: add new rules by extending `BLOCKED_PATTERNS` or `BLOCKED_BINARIES`
/// without modifying `RunTool`.
pub struct SecurityPolicy;

/// Result of a security check.
#[derive(Debug)]
pub enum SecurityVerdict {
    /// Command is allowed to execute.
    Allow,
    /// Command is blocked with a reason.
    Block(String),
}

/// Patterns that indicate privilege escalation or destructive operations.
const BLOCKED_PATTERNS: &[(&str, &str)] = &[
    ("sudo ", "privilege escalation via sudo is not allowed"),
    ("su ", "privilege escalation via su is not allowed"),
    ("sudo\t", "privilege escalation via sudo is not allowed"),
    (" | sudo", "piping into sudo is not allowed"),
];

/// Binaries that are never allowed to be executed directly.
const BLOCKED_BINARIES: &[(&str, &str)] = &[
    ("sudo", "privilege escalation via sudo is not allowed"),
    ("su", "privilege escalation via su is not allowed"),
    ("doas", "privilege escalation via doas is not allowed"),
    ("pkexec", "privilege escalation via pkexec is not allowed"),
];

/// Dangerous commands that require extra caution (shown in confirmation).
const DESTRUCTIVE_PATTERNS: &[&str] = &[
    "rm -rf",
    "rm -r",
    "mkfs",
    "dd if=",
    ":(){:|:&};:",
    "chmod -R 777",
    "format ",
    "> /dev/",
];

impl SecurityPolicy {
    /// Validate a raw command string before parsing/execution.
    pub fn validate_raw(command: &str) -> SecurityVerdict {
        let trimmed = command.trim();

        // Check blocked patterns in raw command
        for (pattern, reason) in BLOCKED_PATTERNS {
            if trimmed.contains(pattern) {
                return SecurityVerdict::Block(reason.to_string());
            }
        }

        // Check if the command starts with a blocked binary
        let first_word = trimmed.split_whitespace().next().unwrap_or("");
        // Strip path prefix: "/usr/bin/sudo" → "sudo"
        let binary_name = first_word.rsplit('/').next().unwrap_or(first_word);

        for (blocked, reason) in BLOCKED_BINARIES {
            if binary_name == *blocked {
                return SecurityVerdict::Block(reason.to_string());
            }
        }

        SecurityVerdict::Allow
    }

    /// Check if a command contains destructive patterns (for UI warnings).
    pub fn is_destructive(command: &str) -> bool {
        let lower = command.to_lowercase();
        DESTRUCTIVE_PATTERNS
            .iter()
            .any(|pattern| lower.contains(pattern))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_safe_commands() {
        assert!(matches!(
            SecurityPolicy::validate_raw("cargo build"),
            SecurityVerdict::Allow
        ));
        assert!(matches!(
            SecurityPolicy::validate_raw("npm test"),
            SecurityVerdict::Allow
        ));
        assert!(matches!(
            SecurityPolicy::validate_raw("git status"),
            SecurityVerdict::Allow
        ));
    }

    #[test]
    fn blocks_sudo() {
        assert!(matches!(
            SecurityPolicy::validate_raw("sudo rm -rf /"),
            SecurityVerdict::Block(_)
        ));
    }

    #[test]
    fn blocks_su() {
        assert!(matches!(
            SecurityPolicy::validate_raw("su root"),
            SecurityVerdict::Block(_)
        ));
    }

    #[test]
    fn blocks_doas() {
        assert!(matches!(
            SecurityPolicy::validate_raw("doas rm -rf /"),
            SecurityVerdict::Block(_)
        ));
    }

    #[test]
    fn blocks_full_path_sudo() {
        assert!(matches!(
            SecurityPolicy::validate_raw("/usr/bin/sudo rm -rf /"),
            SecurityVerdict::Block(_)
        ));
    }

    #[test]
    fn blocks_pipe_to_sudo() {
        assert!(matches!(
            SecurityPolicy::validate_raw("echo password | sudo -S rm -rf /"),
            SecurityVerdict::Block(_)
        ));
    }

    #[test]
    fn detects_destructive_patterns() {
        assert!(SecurityPolicy::is_destructive("rm -rf /tmp/test"));
        assert!(SecurityPolicy::is_destructive("dd if=/dev/zero of=/dev/sda"));
        assert!(!SecurityPolicy::is_destructive("cargo build"));
    }
}
