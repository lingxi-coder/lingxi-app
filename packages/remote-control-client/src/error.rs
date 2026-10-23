//! Errors shared by the codec layers.

/// The peer (or the data store it writes to) broke the `lingxi-rc/1` wire rules:
/// a malformed fragment, frame, record or encoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ProtocolError(pub String);

impl ProtocolError {
    /// Build an error from any message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}
