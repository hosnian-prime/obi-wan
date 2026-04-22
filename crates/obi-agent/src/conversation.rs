use obi_llm::provider::Message;

use crate::token::estimate_tokens;

/// Manages conversation history with token budget trimming.
/// Keeps messages within the LLM's context window by removing
/// the oldest messages (except the system/first user message) when over budget.
pub struct ConversationHistory {
    messages: Vec<Message>,
    /// Max tokens allocated for conversation history.
    max_tokens: usize,
}

impl ConversationHistory {
    pub fn new(max_tokens: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_tokens,
        }
    }

    /// Add a message to the history.
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
        self.trim();
    }

    /// Get all messages in the history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Current token usage.
    pub fn token_count(&self) -> usize {
        self.messages
            .iter()
            .map(|m| estimate_tokens(&m.content))
            .sum()
    }

    /// Trim old messages to stay within budget.
    /// Strategy: keep the first message (initial user query for context)
    /// and remove from the front of the remaining messages.
    fn trim(&mut self) {
        while self.token_count() > self.max_tokens && self.messages.len() > 2 {
            // Remove the second message (keep first as context anchor)
            self.messages.remove(1);
        }
    }

    /// Clear all messages.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_push_and_get() {
        let mut history = ConversationHistory::new(10000);
        history.push(Message::user("hello"));
        history.push(Message::assistant("hi there"));
        assert_eq!(history.len(), 2);
        assert_eq!(history.messages()[0].content, "hello");
    }

    #[test]
    fn test_conversation_trimming() {
        // Very small budget to force trimming
        let mut history = ConversationHistory::new(10);
        history.push(Message::user("first"));
        history.push(Message::assistant("second message that is longer"));
        history.push(Message::user("third message that is also long"));

        // Should have trimmed old messages
        // First message is always kept
        assert!(history.messages()[0].content == "first");
        assert!(history.token_count() <= 10 || history.len() <= 2);
    }

    #[test]
    fn test_conversation_clear() {
        let mut history = ConversationHistory::new(10000);
        history.push(Message::user("hello"));
        history.clear();
        assert!(history.is_empty());
    }
}
