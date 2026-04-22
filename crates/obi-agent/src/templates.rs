/// Note templates for the Second Brain system.
///
/// Templates provide pre-structured markdown for common note types:
/// - Architecture Decision Records (ADR)
/// - Bug Investigation notes
/// - Meeting notes
/// - Code Review notes

/// Template metadata.
pub struct NoteTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub content: &'static str,
}

/// All available templates.
pub fn all_templates() -> Vec<NoteTemplate> {
    vec![
        NoteTemplate {
            name: "ADR",
            description: "Architecture Decision Record",
            content: ADR_TEMPLATE,
        },
        NoteTemplate {
            name: "Bug",
            description: "Bug Investigation",
            content: BUG_TEMPLATE,
        },
        NoteTemplate {
            name: "Review",
            description: "Code Review Notes",
            content: REVIEW_TEMPLATE,
        },
    ]
}

/// Get a template by name (case-insensitive).
pub fn get_template(name: &str) -> Option<NoteTemplate> {
    all_templates()
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case(name))
}

const ADR_TEMPLATE: &str = r#"# ADR: [Title]

## Status
Proposed | Accepted | Deprecated | Superseded

## Context
What is the issue that we're seeing that is motivating this decision?

## Decision
What is the change that we're proposing and/or doing?

## Consequences
What becomes easier or more difficult to do because of this change?

## References
- [[related_module]]
- [[related_function]]
"#;

const BUG_TEMPLATE: &str = r#"# Bug: [Title]

## Symptoms
What is the observed behavior?

## Expected Behavior
What should happen instead?

## Steps to Reproduce
1.
2.
3.

## Root Cause
What is causing this? Link to relevant code:
- [[suspect_function]]

## Fix
What was the fix? What alternatives were considered?

## Prevention
How can we prevent this class of bug in the future?
"#;

const REVIEW_TEMPLATE: &str = r#"# Review: [Title]

## Summary
Brief description of what's being reviewed.

## Key Changes
-

## Concerns
-

## Related Code
- [[function_or_module]]

## Decision
Approved | Changes Requested | Needs Discussion
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_templates() {
        let templates = all_templates();
        assert_eq!(templates.len(), 3);
        assert_eq!(templates[0].name, "ADR");
        assert_eq!(templates[1].name, "Bug");
        assert_eq!(templates[2].name, "Review");
    }

    #[test]
    fn test_get_template_case_insensitive() {
        assert!(get_template("adr").is_some());
        assert!(get_template("ADR").is_some());
        assert!(get_template("bug").is_some());
        assert!(get_template("nonexistent").is_none());
    }

    #[test]
    fn test_templates_contain_wiki_links() {
        let adr = get_template("ADR").unwrap();
        assert!(adr.content.contains("[["));
        let bug = get_template("Bug").unwrap();
        assert!(bug.content.contains("[[suspect_function]]"));
    }
}
