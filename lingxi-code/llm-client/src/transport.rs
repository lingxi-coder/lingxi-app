//! Transport-neutral request and response primitives.

use std::collections::BTreeMap;

/// HTTP request after protocol and endpoint preparation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    /// Absolute request URL.
    pub url: String,
    /// Header map ready for authentication and transport.
    pub headers: BTreeMap<String, String>,
    /// Serialized request body.
    pub body: Vec<u8>,
}

impl PreparedRequest {
    /// Create a prepared request with no headers or body.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        }
    }

    /// Attach a serialized request body.
    #[must_use]
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }
}
