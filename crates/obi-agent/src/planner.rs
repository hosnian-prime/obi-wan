use anyhow::Result;

use obi_llm::provider::{CompletionProvider, CompletionRequest, Message};

/// A single step in the agent's execution plan.
#[derive(Debug, Clone)]
pub struct PlanStep {
    pub description: String,
    pub tool_hint: Option<String>,
    pub completed: bool,
}

/// An execution plan — a sequence of steps the agent intends to follow.
#[derive(Debug, Clone)]
pub struct Plan {
    pub steps: Vec<PlanStep>,
    pub reasoning: String,
}

impl Plan {
    pub fn mark_completed(&mut self, index: usize) {
        if let Some(step) = self.steps.get_mut(index) {
            step.completed = true;
        }
    }

    pub fn next_pending(&self) -> Option<(usize, &PlanStep)> {
        self.steps
            .iter()
            .enumerate()
            .find(|(_, s)| !s.completed)
    }

    pub fn all_completed(&self) -> bool {
        self.steps.iter().all(|s| s.completed)
    }

    /// Format plan as a human-readable summary.
    pub fn summary(&self) -> String {
        let mut lines = vec![format!("Plan: {}", self.reasoning)];
        for (i, step) in self.steps.iter().enumerate() {
            let marker = if step.completed { "[x]" } else { "[ ]" };
            let tool = step
                .tool_hint
                .as_ref()
                .map(|t| format!(" ({})", t))
                .unwrap_or_default();
            lines.push(format!("  {} {}. {}{}", marker, i + 1, step.description, tool));
        }
        lines.join("\n")
    }
}

/// The Planner breaks a user request into executable steps by asking the LLM.
pub struct Planner;

/// System prompt for the planner — instructs the LLM to output a structured plan.
const PLANNER_SYSTEM: &str = r#"You are a planning assistant for a code editor AI agent. Given a user request and code context, break the request into concrete, executable steps.

Output your plan as a numbered list. Each step should be a single action. For each step, optionally indicate which tool would be used in parentheses.

Available tools: read (read files), edit (modify files), search (find patterns), run (execute commands), graph_query (query code graph).

Format:
REASONING: <one sentence explaining your approach>
1. <step description> (tool_name)
2. <step description> (tool_name)
...

If the request is a simple question that can be answered directly from the provided context, output:
REASONING: <explanation>
1. Answer the question directly

Keep plans concise — typically 1-5 steps."#;

impl Planner {
    /// Generate an execution plan for the given user query.
    /// Uses the LLM to break the request into steps.
    pub async fn plan(
        provider: &dyn CompletionProvider,
        user_query: &str,
        context: &str,
    ) -> Result<Plan> {
        let prompt = if context.is_empty() {
            format!("User request: {}", user_query)
        } else {
            format!(
                "Code context:\n{}\n\nUser request: {}",
                context, user_query
            )
        };

        let req = CompletionRequest {
            system: Some(PLANNER_SYSTEM.to_string()),
            messages: vec![Message::user(prompt)],
            max_tokens: 1024,
            tools: Vec::new(),
            temperature: 0.1,
        };

        let response = provider.complete(req).await?;
        parse_plan(&response.content)
    }
}

/// Parse the LLM's plan output into a structured Plan.
fn parse_plan(text: &str) -> Result<Plan> {
    let mut reasoning = String::new();
    let mut steps = Vec::new();

    for line in text.lines() {
        let line = line.trim();

        if line.starts_with("REASONING:") {
            reasoning = line.trim_start_matches("REASONING:").trim().to_string();
            continue;
        }

        // Match numbered steps: "1. description (tool_name)"
        if let Some(rest) = strip_number_prefix(line) {
            let (description, tool_hint) = extract_tool_hint(rest);
            steps.push(PlanStep {
                description,
                tool_hint,
                completed: false,
            });
        }
    }

    // Fallback: if no structured plan was parsed, create a single "answer directly" step
    if steps.is_empty() {
        steps.push(PlanStep {
            description: "Process the request".to_string(),
            tool_hint: None,
            completed: false,
        });
        if reasoning.is_empty() {
            reasoning = "Direct response".to_string();
        }
    }

    Ok(Plan { steps, reasoning })
}

/// Strip a leading number + period from a line: "1. foo" -> Some("foo")
fn strip_number_prefix(line: &str) -> Option<&str> {
    let line = line.trim();
    let dot_pos = line.find('.')?;
    let prefix = &line[..dot_pos];
    if prefix.chars().all(|c| c.is_ascii_digit()) && dot_pos < line.len() {
        Some(line[dot_pos + 1..].trim())
    } else {
        None
    }
}

/// Extract optional tool hint from parentheses at end: "foo (read)" -> ("foo", Some("read"))
fn extract_tool_hint(text: &str) -> (String, Option<String>) {
    let text = text.trim();
    if let Some(open) = text.rfind('(') {
        if text.ends_with(')') {
            let tool = text[open + 1..text.len() - 1].trim().to_string();
            let desc = text[..open].trim().to_string();
            // Only treat as tool hint if it's a known tool name
            if ["read", "edit", "search", "run", "graph_query"].contains(&tool.as_str()) {
                return (desc, Some(tool));
            }
        }
    }
    (text.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_plan_basic() {
        let text = "REASONING: Need to read and modify the file\n\
                     1. Read the current implementation (read)\n\
                     2. Apply the fix to the parse function (edit)\n\
                     3. Run tests to verify (run)";

        let plan = parse_plan(text).unwrap();
        assert_eq!(plan.reasoning, "Need to read and modify the file");
        assert_eq!(plan.steps.len(), 3);
        assert_eq!(plan.steps[0].tool_hint, Some("read".to_string()));
        assert_eq!(plan.steps[1].tool_hint, Some("edit".to_string()));
        assert_eq!(plan.steps[2].tool_hint, Some("run".to_string()));
        assert!(!plan.steps[0].completed);
    }

    #[test]
    fn test_parse_plan_no_tools() {
        let text = "REASONING: Simple question\n\
                     1. Answer the question directly";

        let plan = parse_plan(text).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].tool_hint, None);
    }

    #[test]
    fn test_parse_plan_fallback() {
        let text = "I'll just help you with that.";
        let plan = parse_plan(text).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].description, "Process the request");
    }

    #[test]
    fn test_plan_summary() {
        let plan = Plan {
            reasoning: "Fix the bug".to_string(),
            steps: vec![
                PlanStep {
                    description: "Read file".to_string(),
                    tool_hint: Some("read".to_string()),
                    completed: true,
                },
                PlanStep {
                    description: "Apply fix".to_string(),
                    tool_hint: Some("edit".to_string()),
                    completed: false,
                },
            ],
        };
        let summary = plan.summary();
        assert!(summary.contains("[x] 1. Read file (read)"));
        assert!(summary.contains("[ ] 2. Apply fix (edit)"));
    }

    #[test]
    fn test_strip_number_prefix() {
        assert_eq!(strip_number_prefix("1. hello"), Some("hello"));
        assert_eq!(strip_number_prefix("12. world"), Some("world"));
        assert_eq!(strip_number_prefix("no number"), None);
        assert_eq!(strip_number_prefix("abc. bad"), None);
    }

    #[test]
    fn test_extract_tool_hint() {
        let (desc, tool) = extract_tool_hint("Read the file (read)");
        assert_eq!(desc, "Read the file");
        assert_eq!(tool, Some("read".to_string()));

        let (desc, tool) = extract_tool_hint("Do something (unknown)");
        assert_eq!(desc, "Do something (unknown)");
        assert_eq!(tool, None);

        let (desc, tool) = extract_tool_hint("No tool here");
        assert_eq!(desc, "No tool here");
        assert_eq!(tool, None);
    }
}
