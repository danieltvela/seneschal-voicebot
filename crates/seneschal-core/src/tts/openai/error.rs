//! Errors raised by the OpenAI-compatible TTS provider (issue #16).

/// Failure modes of the remote TTS provider.
#[derive(Debug, thiserror::Error)]
pub enum OpenAiTtsError {
    /// Could not reach the service (connection refused, DNS failure, ...).
    #[error("TTS service unreachable: {0}")]
    Connection(String),
    /// The service returned a non-success HTTP status. `message` is the
    /// human-readable detail extracted from the OpenAI-style error body.
    #[error("TTS service error (HTTP {status}): {message}")]
    Service { status: u16, message: String },
    /// The request or stream read exceeded the configured timeout.
    #[error("TTS request timed out: {0}")]
    Timeout(String),
    /// The response body could not be decoded.
    #[error("TTS response decode error: {0}")]
    Decode(String),
    /// The request was rejected locally before reaching the service.
    #[error("TTS request rejected: {0}")]
    Invalid(String),
}

/// Convenience result alias for the provider.
pub type OpenAiTtsResult<T> = Result<T, OpenAiTtsError>;
