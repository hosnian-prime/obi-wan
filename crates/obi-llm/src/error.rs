use thiserror::Error;

/// LLM provider errors with classification for router fallback logic.
#[derive(Debug, Error)]
pub enum LlmError {
    /// Rate limited by the provider (HTTP 429).
    #[error("rate limited: {0}")]
    RateLimit(String),

    /// Server error from the provider (HTTP 5xx).
    #[error("server error: {0}")]
    ServerError(String),

    /// Authentication failure (HTTP 401/403).
    #[error("auth error: {0}")]
    AuthError(String),

    /// Request was invalid (HTTP 400).
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Network/connection error.
    #[error("connection error: {0}")]
    ConnectionError(String),

    /// Any other error.
    #[error("{0}")]
    Other(String),
}

impl LlmError {
    /// Whether this error should trigger a fallback to another provider.
    pub fn is_retriable(&self) -> bool {
        matches!(self, LlmError::RateLimit(_) | LlmError::ServerError(_))
    }

    pub fn is_rate_limit(&self) -> bool {
        matches!(self, LlmError::RateLimit(_))
    }

    pub fn is_server_error(&self) -> bool {
        matches!(self, LlmError::ServerError(_))
    }

    /// Classify an HTTP status code into the appropriate error variant.
    pub fn from_status(status: u16, body: String) -> Self {
        match status {
            429 => LlmError::RateLimit(body),
            401 | 403 => LlmError::AuthError(body),
            400 => LlmError::BadRequest(body),
            500..=599 => LlmError::ServerError(body),
            _ => LlmError::Other(format!("HTTP {}: {}", status, body)),
        }
    }
}

impl From<reqwest::Error> for LlmError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() || err.is_connect() {
            LlmError::ConnectionError(err.to_string())
        } else if let Some(status) = err.status() {
            LlmError::from_status(status.as_u16(), err.to_string())
        } else {
            LlmError::Other(err.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_classification() {
        assert!(LlmError::RateLimit("too many".into()).is_retriable());
        assert!(LlmError::ServerError("500".into()).is_retriable());
        assert!(!LlmError::BadRequest("bad".into()).is_retriable());
        assert!(!LlmError::AuthError("unauth".into()).is_retriable());
    }

    #[test]
    fn test_from_status() {
        assert!(LlmError::from_status(429, "".into()).is_rate_limit());
        assert!(LlmError::from_status(500, "".into()).is_server_error());
        assert!(LlmError::from_status(503, "".into()).is_server_error());
    }
}
