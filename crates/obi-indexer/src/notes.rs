use obi_core::node::NodeType;

use crate::parser::RawNode;

/// Parse markdown file and extract [[links]] plus create a node for the note itself.
pub fn parse_note(source: &str, file_name: &str) -> (RawNode, Vec<String>) {
    let links = extract_wiki_links(source);

    let node = RawNode {
        name: file_name.trim_end_matches(".md").to_string(),
        node_type: NodeType::Note,
        content: source.to_string(),
        line_start: 0,
        line_end: source.lines().count(),
    };

    (node, links)
}

/// Extract all [[link]] targets from markdown text.
fn extract_wiki_links(source: &str) -> Vec<String> {
    let mut links = Vec::new();
    let bytes = source.as_bytes();
    let mut i = 0;

    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            i += 2;
            let start = i;
            while i + 1 < bytes.len() {
                if bytes[i] == b']' && bytes[i + 1] == b']' {
                    let link = &source[start..i];
                    if !link.is_empty() {
                        links.push(link.to_string());
                    }
                    i += 2;
                    break;
                }
                i += 1;
            }
        } else {
            i += 1;
        }
    }

    links
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_wiki_links() {
        let source = "See [[parse_function]] and [[tokenize]] for details.";
        let links = extract_wiki_links(source);
        assert_eq!(links, vec!["parse_function", "tokenize"]);
    }

    #[test]
    fn test_no_links() {
        let source = "No links here.";
        let links = extract_wiki_links(source);
        assert!(links.is_empty());
    }

    #[test]
    fn test_parse_note() {
        let source = "# My Note\n\nSee [[foo]] and [[bar]].\n";
        let (node, links) = parse_note(source, "my_note.md");
        assert_eq!(node.name, "my_note");
        assert_eq!(links, vec!["foo", "bar"]);
    }
}
