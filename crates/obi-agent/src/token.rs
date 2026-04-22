/// Estimate token count for a piece of text.
/// MVP heuristic: ~1 token per 4 characters (~80% accurate).
/// Future: provider-specific tokenizers via tiktoken-rs.
pub fn estimate_tokens(text: &str) -> usize {
    // +1 to avoid zero for very short strings
    (text.len() + 3) / 4
}

/// Minimum remaining budget before we stop packing nodes.
pub const MIN_REMAINING_TOKENS: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hi"), 1);
        assert_eq!(estimate_tokens("hello world"), 3); // 11 chars -> ~3 tokens
        // 100 chars -> 25 tokens
        let hundred = "a".repeat(100);
        assert_eq!(estimate_tokens(&hundred), 25);
    }

    #[test]
    fn test_short_strings_nonzero() {
        // Single char should give 1, not 0
        assert_eq!(estimate_tokens("x"), 1);
        assert_eq!(estimate_tokens("ab"), 1);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
